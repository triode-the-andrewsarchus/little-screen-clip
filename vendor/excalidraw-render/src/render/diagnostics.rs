use crate::diagnostic_registry::{
    DiagnosticKind, FallbackRecipe, ImageFailure, ImageRecovery, LocalLimit, LocalRecovery,
    MetadataFailure,
};
use crate::normalization::{NormalizedElement, NormalizedElementKind, NormalizedImageStatus};
use crate::outcome::PendingDiagnostic;
use crate::{
    CompatibilityDiagnostic, DiagnosticCode, DiagnosticDetails, DiagnosticEffect, OutputFormat,
    ResourceLimits, ResourceValue,
};

#[cfg(feature = "pdf")]
use super::pdf;
use super::{asset, try_owned};

/// Orders retained diagnostics the way the final report lists them (spec
/// §11.3): by the source owner that produced them, then the source member
/// position within that owner, then the full nested path, pointer and code's stable
/// spelling, and finally by the order in which they were retained, so equal
/// keys keep the retention order and the sort is total.
pub(crate) fn sort_diagnostics(diagnostics: &mut [CompatibilityDiagnostic]) {
    diagnostics.sort_unstable_by(|left, right| {
        let (left_order, right_order) = (left.order(), right.order());
        left_order
            .owner_index
            .cmp(&right_order.owner_index)
            .then_with(|| left_order.source_position.cmp(&right_order.source_position))
            .then_with(|| compare_nested_paths(left, right))
            .then_with(|| left.json_pointer().cmp(right.json_pointer()))
            .then_with(|| left.code().as_str().cmp(right.code().as_str()))
            .then_with(|| {
                left_order
                    .retention_ordinal
                    .cmp(&right_order.retention_ordinal)
            })
    });
}

fn compare_nested_paths(
    left: &CompatibilityDiagnostic,
    right: &CompatibilityDiagnostic,
) -> std::cmp::Ordering {
    fn paths(diagnostic: &CompatibilityDiagnostic) -> &[crate::NestedPath] {
        match diagnostic.details() {
            Some(
                DiagnosticDetails::NestedSource { paths, .. }
                | DiagnosticDetails::NestedAssetClaim { paths, .. }
                | DiagnosticDetails::NestedResourceLimit { paths, .. },
            ) => paths,
            _ => &[],
        }
    }
    let (left, right) = (paths(left), paths(right));
    for (left, right) in left.iter().zip(right) {
        let key = |segment: &crate::NestedPathSegment| {
            let kind = match segment.kind() {
                crate::NestedSegmentKind::Node => 0,
                crate::NestedSegmentKind::Attribute => 1,
                crate::NestedSegmentKind::CssRule => 2,
                crate::NestedSegmentKind::CssDeclaration => 3,
                crate::NestedSegmentKind::ResourceSlot => 4,
                crate::NestedSegmentKind::EffectInput => 5,
            };
            (segment.ordinal(), kind)
        };
        let order = left
            .segments()
            .iter()
            .map(key)
            .cmp(right.segments().iter().map(key));
        if !order.is_eq() {
            return order;
        }
    }
    left.len().cmp(&right.len())
}

/// Whether recovery replaces this entire source, rather than just one field.
pub(super) fn diagnostic_replaces_source(
    source: &NormalizedElement,
    diagnostic: &CompatibilityDiagnostic,
) -> bool {
    if !diagnostic.replaces_occurrence() {
        return false;
    }
    let applies = diagnostic.element_id() == Some(source.source.id())
        || matches!(
            (&source.kind, diagnostic.details()),
            (
                NormalizedElementKind::Image(image),
                Some(DiagnosticDetails::Asset { asset_id, .. })
            ) if image.file_id.as_deref() == Some(asset_id.as_str())
        )
        || image_payload_field_recovery(source, diagnostic);
    if !applies {
        return false;
    }
    if diagnostic.code() == DiagnosticCode::COMPATIBILITY_UNSUPPORTED_PROPERTY_VALUE
        && diagnostic.details().and_then(DiagnosticDetails::recovery)
            == Some(crate::Recovery::Omitted)
    {
        // Invalid bindings/crop members omit a field, not its owner. Only the
        // points subtree can omit an unavailable linear/freehand element;
        // an authoritative file payload omission replaces its image occurrence.
        return image_payload_field_recovery(source, diagnostic)
            || matches!(
                source.kind,
                NormalizedElementKind::Linear(_) | NormalizedElementKind::Freedraw(_)
            ) && diagnostic
                .json_pointer()
                .strip_prefix(source.source.pointer())
                .is_some_and(|field| field == "/points" || field.starts_with("/points/"));
    }
    true
}

/// Whether a retained diagnostic replaced `source` (or its asset) with an
/// official placeholder or omission, rather than the generic known-element box.
pub(super) fn diagnostic_uses_non_generic_recovery(
    source: &NormalizedElement,
    diagnostic: &CompatibilityDiagnostic,
) -> bool {
    if !diagnostic_replaces_source(source, diagnostic) {
        return false;
    }
    let recipe = diagnostic
        .details()
        .and_then(DiagnosticDetails::recipe_id)
        .and_then(FallbackRecipe::from_id);
    recipe != Some(FallbackRecipe::KnownElementUnavailable)
}

/// A source-owned file field can reject an image payload without carrying
/// asset-shaped details. Match the exact escaped payload pointer so another
/// file or an unrelated field cannot suppress this occurrence's failure.
pub(super) fn image_payload_field_recovery(
    source: &NormalizedElement,
    diagnostic: &CompatibilityDiagnostic,
) -> bool {
    if diagnostic.code() != DiagnosticCode::COMPATIBILITY_UNSUPPORTED_PROPERTY_VALUE
        || !diagnostic.replaces_occurrence()
    {
        return false;
    }
    let NormalizedElementKind::Image(image) = &source.kind else {
        return false;
    };
    let Some(escaped_id) = diagnostic
        .json_pointer()
        .strip_prefix("/files/")
        .and_then(|pointer| pointer.strip_suffix("/dataURL"))
    else {
        return false;
    };
    image.file_id.as_deref().is_some_and(|id| {
        // Compare the RFC 6901 spelling without allocating. Allocation refusal
        // must never masquerade as a missing cause and enable generic recovery.
        let mut encoded = escaped_id.bytes();
        id.bytes().all(|byte| match byte {
            b'~' => encoded.next() == Some(b'~') && encoded.next() == Some(b'0'),
            b'/' => encoded.next() == Some(b'~') && encoded.next() == Some(b'1'),
            _ => encoded.next() == Some(byte),
        }) && encoded.next().is_none()
    })
}

pub(super) fn known_element_replaced_diagnostic(
    format: OutputFormat,
    element_id: &str,
    json_pointer: &str,
    has_bounds: bool,
) -> Result<PendingDiagnostic, std::collections::TryReserveError> {
    PendingDiagnostic::new(
        DiagnosticKind::KnownElementReplaced(LocalRecovery::placeholder_if(has_bounds)),
        format,
        &[DiagnosticEffect::Visual, DiagnosticEffect::Semantic],
        Some(element_id),
        json_pointer,
        "known element unavailable to this renderer used the revision-1 fallback",
    )
}

/// The registry failure and non-normative message for an asset resolution
/// error the image resolver reports as a diagnostic. Missing payloads are
/// diagnosed by normalization and memory exhaustion is fatal, so neither
/// reaches here.
fn image_failure(
    failure: asset::ResolveError,
    limits: &ResourceLimits,
) -> (ImageFailure, &'static str) {
    let limit_crossed =
        |limit: LocalLimit, configured: u64, attempted: u64| ImageFailure::LimitCrossed {
            limit,
            configured: ResourceValue::Integer(configured),
            attempted: crate::Measurement::Attempted {
                value: ResourceValue::Integer(attempted),
            },
        };
    match failure {
        asset::ResolveError::Forbidden => (
            ImageFailure::ForbiddenResource,
            "non-local resource was rejected without fetching",
        ),
        asset::ResolveError::Coordinate { value_bits } => (
            ImageFailure::LimitCrossed {
                limit: LocalLimit::CoordinateMagnitude,
                configured: ResourceValue::Finite(
                    crate::FiniteValue::new(limits.max_coordinate_magnitude())
                        .expect("admitted limit"),
                ),
                attempted: super::coordinates::measurement(
                    f64::from_bits(value_bits),
                    limits.max_coordinate_magnitude(),
                ),
            },
            "nested SVG exceeded the coordinate ceiling",
        ),
        asset::ResolveError::Malformed => (
            ImageFailure::MalformedFileRecord,
            "referenced authoritative file payload was malformed",
        ),
        asset::ResolveError::Unsupported => (
            ImageFailure::UnsupportedFormat,
            "referenced image payload was not a supported raster format",
        ),
        asset::ResolveError::DecodeFailed => (
            ImageFailure::DecodeFailed {
                media_type: "image/png",
            },
            "referenced static PNG payload could not be decoded",
        ),
        asset::ResolveError::JpegDecodeFailed => (
            ImageFailure::DecodeFailed {
                media_type: "image/jpeg",
            },
            "referenced JPEG/JFIF payload could not be decoded",
        ),
        asset::ResolveError::GifDecodeFailed => (
            ImageFailure::DecodeFailed {
                media_type: "image/gif",
            },
            "referenced GIF payload could not be decoded",
        ),
        asset::ResolveError::WebPDecodeFailed => (
            ImageFailure::DecodeFailed {
                media_type: "image/webp",
            },
            "referenced WebP payload could not be decoded",
        ),
        asset::ResolveError::BmpDecodeFailed => (
            ImageFailure::DecodeFailed {
                media_type: "image/bmp",
            },
            "referenced BMP payload could not be decoded",
        ),
        asset::ResolveError::IcoDecodeFailed => (
            ImageFailure::DecodeFailed {
                media_type: "image/x-icon",
            },
            "referenced ICO payload could not be decoded",
        ),
        asset::ResolveError::SvgDecodeFailed => (
            ImageFailure::DecodeFailed {
                media_type: "image/svg+xml",
            },
            "referenced nested SVG payload could not be parsed and sanitized",
        ),
        asset::ResolveError::Missing => {
            unreachable!("missing assets are diagnosed by normalization")
        }
        asset::ResolveError::AssetBytes { attempted } => (
            limit_crossed(LocalLimit::AssetBytes, limits.max_asset_bytes(), attempted),
            "asset payload occurrence exceeded its byte ceiling",
        ),
        asset::ResolveError::Dimensions { attempted } => (
            limit_crossed(
                LocalLimit::RasterDimension,
                u64::from(limits.max_raster_dimension()),
                attempted,
            ),
            "raster dimensions exceeded the local image ceiling",
        ),
        asset::ResolveError::Pixels { attempted } => (
            limit_crossed(
                LocalLimit::DecodedPixelsPerAsset,
                limits.max_decoded_pixels_per_asset(),
                attempted,
            ),
            "raster pixels exceeded the local asset ceiling",
        ),
        asset::ResolveError::AssetWork { attempted } => (
            limit_crossed(
                LocalLimit::AssetWorkUnits,
                limits.max_asset_work_units(),
                attempted,
            ),
            "raster intrinsic work exceeded the local asset ceiling",
        ),
        asset::ResolveError::SvgComplexity { attempted } => (
            limit_crossed(
                LocalLimit::NestedSvgNodes,
                u64::from(limits.max_nested_svg_nodes()),
                attempted,
            ),
            "nested SVG exceeded the local node ceiling",
        ),
        asset::ResolveError::SvgDepth { attempted } => (
            limit_crossed(
                LocalLimit::NestedAssetDepth,
                u64::from(limits.max_nested_asset_depth()),
                attempted,
            ),
            "nested SVG resource exceeded the local asset-depth ceiling",
        ),
        asset::ResolveError::Memory => unreachable!("memory failure is fatal before diagnostics"),
    }
}

pub(super) fn image_asset_failure_diagnostic(
    failure: asset::ResolveError,
    format: OutputFormat,
    asset_id: &str,
    element_id: &str,
    json_pointer: &str,
    status: NormalizedImageStatus,
    limits: &ResourceLimits,
) -> Result<PendingDiagnostic, std::collections::TryReserveError> {
    let intrinsic_raster_limit = matches!(
        failure,
        asset::ResolveError::Dimensions { .. } | asset::ResolveError::Pixels { .. }
    );
    let coordinate_semantics =
        matches!(failure, asset::ResolveError::Coordinate { .. }) && format != OutputFormat::Png;
    let (failure, message) = image_failure(failure, limits);
    PendingDiagnostic::new(
        DiagnosticKind::ImageUnavailable {
            failure,
            asset_id,
            recovery: ImageRecovery::for_format(format, status == NormalizedImageStatus::Error),
        },
        format,
        if coordinate_semantics {
            &[DiagnosticEffect::Visual, DiagnosticEffect::Semantic]
        } else {
            &[DiagnosticEffect::Visual]
        },
        if intrinsic_raster_limit {
            None
        } else {
            Some(element_id)
        },
        json_pointer,
        message,
    )
}

pub(super) fn image_animation_flattened_diagnostic(
    format: OutputFormat,
    asset_id: &str,
    element_id: &str,
    json_pointer: &str,
    media_type: &str,
) -> Result<PendingDiagnostic, std::collections::TryReserveError> {
    PendingDiagnostic::new(
        DiagnosticKind::AnimationFlattened {
            asset_id,
            media_type,
        },
        format,
        &[DiagnosticEffect::Visual],
        Some(element_id),
        json_pointer,
        "animated image was flattened to its first animation frame",
    )
}

#[cfg(feature = "pdf")]
pub(super) fn pdf_raster_fallback_diagnostic(
    closure: &pdf::PdfRasterClosure,
) -> Result<PendingDiagnostic, std::collections::TryReserveError> {
    PendingDiagnostic::new(
        DiagnosticKind::PdfRasterFallback {
            logical_bounds: closure.logical_bounds,
            pixel_width: closure.pixel_width,
            pixel_height: closure.pixel_height,
            causes: closure.causes.clone(),
        },
        OutputFormat::Pdf,
        &[DiagnosticEffect::Visual, DiagnosticEffect::Semantic],
        Some(&closure.element_id),
        &closure.json_pointer,
        "unsupported nested-SVG effect closure was rasterized for PDF",
    )
}

/// The one-entry asset chain of a nested-source diagnostic: the scene asset
/// `asset_id` at its first occurrence.
fn nested_asset_chain(
    asset_id: &str,
) -> Result<Vec<crate::AssetChainEntry>, std::collections::TryReserveError> {
    let mut asset_chain = Vec::new();
    asset_chain.try_reserve_exact(1)?;
    asset_chain.push(crate::AssetChainEntry {
        asset_id: Some(try_owned(asset_id)?),
        occurrence: 0,
    });
    Ok(asset_chain)
}

pub(super) fn unsafe_svg_removal_diagnostic(
    format: OutputFormat,
    asset_id: &str,
    json_pointer: &str,
    removal: &asset::svg::Removal,
) -> Result<PendingDiagnostic, std::collections::TryReserveError> {
    let effects: &[DiagnosticEffect] = if removal.changes_appearance {
        &[DiagnosticEffect::Visual, DiagnosticEffect::Semantic]
    } else {
        &[DiagnosticEffect::Semantic]
    };
    let mut segments = Vec::new();
    segments.try_reserve_exact(
        removal.slots.len().saturating_mul(2)
            + if removal.attribute_ordinal.is_some() {
                2
            } else {
                1
            },
    )?;
    for (node_ordinal, attribute_ordinal) in &removal.slots {
        segments.push(crate::NestedPathSegment {
            kind: crate::NestedSegmentKind::Node,
            ordinal: *node_ordinal,
            name: None,
        });
        segments.push(crate::NestedPathSegment {
            kind: crate::NestedSegmentKind::Attribute,
            ordinal: *attribute_ordinal,
            name: Some(try_owned("href")?),
        });
    }
    segments.push(crate::NestedPathSegment {
        kind: crate::NestedSegmentKind::Node,
        ordinal: removal.node_ordinal,
        name: None,
    });
    if let Some(ordinal) = removal.attribute_ordinal {
        segments.push(crate::NestedPathSegment {
            kind: crate::NestedSegmentKind::Attribute,
            ordinal,
            name: Some(try_owned(&removal.name)?),
        });
    }
    let mut paths = Vec::new();
    paths.try_reserve_exact(1)?;
    paths.push(crate::NestedPath { segments });
    PendingDiagnostic::new(
        DiagnosticKind::UnsafeSvgContentRemoved {
            asset_chain: nested_asset_chain(asset_id)?,
            paths,
        },
        format,
        effects,
        None,
        json_pointer,
        "unsafe nested SVG content was removed before rendering",
    )
}

pub(super) fn nested_media_claim_diagnostic(
    format: OutputFormat,
    asset_id: &str,
    pointer: &str,
    claim: &asset::svg::MediaClaim,
) -> Result<PendingDiagnostic, std::collections::TryReserveError> {
    let mut segments = Vec::new();
    segments.try_reserve_exact(claim.path.len().saturating_mul(2))?;
    for (node, attribute) in &claim.path {
        segments.push(crate::NestedPathSegment {
            kind: crate::NestedSegmentKind::Node,
            ordinal: *node,
            name: None,
        });
        segments.push(crate::NestedPathSegment {
            kind: crate::NestedSegmentKind::Attribute,
            ordinal: *attribute,
            name: Some(try_owned("href")?),
        });
    }
    let mut paths = Vec::new();
    paths.try_reserve_exact(1)?;
    paths.push(crate::NestedPath { segments });
    PendingDiagnostic::new(
        DiagnosticKind::NestedMediaTypeMismatch {
            asset_chain: nested_asset_chain(asset_id)?,
            paths,
            claimed_media_type: try_owned(&claim.claimed)?,
            validated_media_type: try_owned(&claim.validated)?,
        },
        format,
        &[DiagnosticEffect::InputRepair],
        None,
        pointer,
        "nested image media claim was repaired to the validated payload type",
    )
}

pub(super) fn nested_image_failure_diagnostic(
    failure: &asset::svg::Failure,
    format: OutputFormat,
    asset_id: &str,
    json_pointer: &str,
    limits: &ResourceLimits,
) -> Result<PendingDiagnostic, std::collections::TryReserveError> {
    let (failure_kind, _) = image_failure(failure.error, limits);
    let mut segments = Vec::new();
    segments.try_reserve_exact(failure.path.len().saturating_mul(2))?;
    for (node_ordinal, attribute_ordinal) in &failure.path {
        segments.push(crate::NestedPathSegment {
            kind: crate::NestedSegmentKind::Node,
            ordinal: *node_ordinal,
            name: None,
        });
        segments.push(crate::NestedPathSegment {
            kind: crate::NestedSegmentKind::Attribute,
            ordinal: *attribute_ordinal,
            name: Some(try_owned("href")?),
        });
    }
    let mut paths = Vec::new();
    paths.try_reserve_exact(1)?;
    paths.push(crate::NestedPath { segments });
    if failure.error == asset::ResolveError::Forbidden {
        return PendingDiagnostic::new(
            DiagnosticKind::NestedForbiddenResource {
                asset_chain: nested_asset_chain(asset_id)?,
                paths,
                recovery: LocalRecovery::placeholder_if(failure.placeholder),
            },
            format,
            &[DiagnosticEffect::Visual, DiagnosticEffect::Semantic],
            None,
            json_pointer,
            "non-local nested resource was rejected without fetching",
        );
    }
    if let ImageFailure::LimitCrossed {
        limit,
        configured,
        attempted,
    } = failure_kind
    {
        return PendingDiagnostic::new(
            DiagnosticKind::NestedResourceLimit {
                limit,
                configured,
                attempted,
                asset_chain: nested_asset_chain(asset_id)?,
                paths,
                recovery: LocalRecovery::placeholder_if(failure.placeholder),
            },
            format,
            if failure.may_lose_semantics && format != OutputFormat::Png {
                &[DiagnosticEffect::Visual, DiagnosticEffect::Semantic]
            } else {
                &[DiagnosticEffect::Visual]
            },
            None,
            json_pointer,
            "nested resource exceeded its semantic limit",
        );
    }
    PendingDiagnostic::new(
        DiagnosticKind::NestedImageRemoved {
            failure: failure_kind,
            asset_chain: nested_asset_chain(asset_id)?,
            paths,
        },
        format,
        &[DiagnosticEffect::Visual],
        None,
        json_pointer,
        "nested data image was removed after local recovery",
    )
}

pub(super) fn relationship_cycle_diagnostic(
    format: OutputFormat,
    element_id: &str,
    json_pointer: &str,
    visual: bool,
    semantic: bool,
) -> Result<PendingDiagnostic, std::collections::TryReserveError> {
    let effects: &[DiagnosticEffect] = match (visual, semantic) {
        (false, false) => &[DiagnosticEffect::InputRepair],
        (true, false) => &[DiagnosticEffect::InputRepair, DiagnosticEffect::Visual],
        (false, true) => &[DiagnosticEffect::InputRepair, DiagnosticEffect::Semantic],
        (true, true) => &[
            DiagnosticEffect::InputRepair,
            DiagnosticEffect::Visual,
            DiagnosticEffect::Semantic,
        ],
    };
    PendingDiagnostic::new(
        DiagnosticKind::RelationshipCycle,
        format,
        effects,
        Some(element_id),
        json_pointer,
        "relationship cycle-closing edge was removed deterministically",
    )
}

/// The font a generated fallback label or a text element was shaped with
/// instead of its source family, or a scalar no face mapped.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum FontFallback {
    /// Ordinary scene text in the local-only Helvetica family (`fontFamily`
    /// 2), shaped with the bundled Liberation Sans substitute.
    HelveticaText,
    /// A compatibility placeholder's generated label, shaped with the bundled
    /// Liberation Sans.
    GeneratedLabel,
    NotoEmoji,
    EmbeddableNotoEmoji,
    MissingGlyph(u32),
}

pub(super) fn fallback_font_diagnostic(
    fallback: FontFallback,
    format: OutputFormat,
    element_id: Option<&str>,
    json_pointer: &str,
) -> Result<PendingDiagnostic, std::collections::TryReserveError> {
    // Non-normative wording (spec 11.1), bound by approved expected reports.
    // Ordinary scene text and generated export text are distinct glossary
    // terms and get distinct messages (#136); the Noto Emoji one names the
    // face that substitution actually used.
    let (kind, message) = match fallback {
        FontFallback::HelveticaText => (
            DiagnosticKind::LiberationSansSubstituted,
            "Helvetica text used bundled Liberation Sans",
        ),
        FontFallback::GeneratedLabel => (
            DiagnosticKind::LiberationSansSubstituted,
            "generated fallback label used bundled Liberation Sans",
        ),
        FontFallback::NotoEmoji => (
            DiagnosticKind::NotoEmojiSubstituted,
            "emoji text used bundled Noto Emoji",
        ),
        FontFallback::EmbeddableNotoEmoji => (
            DiagnosticKind::EmbeddableNotoEmojiSubstituted,
            "emoji text used bundled Noto Emoji",
        ),
        FontFallback::MissingGlyph(codepoint) => (
            DiagnosticKind::MissingGlyph { codepoint },
            "generated fallback label used the deterministic missing-glyph replacement",
        ),
    };
    PendingDiagnostic::new(
        kind,
        format,
        &[DiagnosticEffect::Visual],
        element_id,
        json_pointer,
        message,
    )
}

pub(super) fn official_missing_glyph_diagnostic(
    format: OutputFormat,
    element_id: &str,
    json_pointer: &str,
    codepoint: u32,
) -> Result<PendingDiagnostic, std::collections::TryReserveError> {
    PendingDiagnostic::new(
        DiagnosticKind::MissingGlyph { codepoint },
        format,
        &[DiagnosticEffect::Visual],
        Some(element_id),
        json_pointer,
        "official text used the deterministic basic-Latin replacement glyph",
    )
}

pub(super) fn known_element_fallback(
    source: &NormalizedElement,
) -> Result<crate::scene_surface::SurfaceFallback, std::collections::TryReserveError> {
    let prefix = "Unavailable: ";
    let mut label = String::new();
    label.try_reserve_exact(prefix.len().saturating_add(source.kind_name.len()))?;
    label.push_str(prefix);
    label.push_str(source.kind_name);
    Ok(crate::scene_surface::SurfaceFallback {
        source_index: source.source.source_index(),
        source_order: source.source_order,
        element_id: Some(try_owned(source.source.id())?),
        json_pointer: try_owned(source.source.pointer())?,
        label,
        link: source.link.as_deref().map(try_owned).transpose()?,
        frame_id: source.frame_id.as_deref().map(try_owned).transpose()?,
        is_frame: matches!(source.kind_name, "frame" | "magicframe"),
        x: source.x,
        y: source.y,
        width: source.width,
        height: source.height,
        angle: source.angle,
        opacity: source.opacity,
        paint: true,
    })
}

pub(super) fn optional_metadata_omitted_diagnostic(
    format: OutputFormat,
    crossing: crate::resource_accounting::OptionalMetadataCrossing,
) -> Result<PendingDiagnostic, std::collections::TryReserveError> {
    PendingDiagnostic::new(
        DiagnosticKind::OptionalMetadataOmitted {
            field: crossing.field,
            configured: crossing.configured,
            value_kind: crossing.value_kind,
            measurement: crossing.measurement,
        },
        format,
        &[DiagnosticEffect::Metadata],
        None,
        "",
        "optional artifact metadata was omitted because it exceeds a resource limit",
    )
}

pub(super) fn metadata_failure_diagnostic(
    format: OutputFormat,
    failure: MetadataFailure,
) -> Result<PendingDiagnostic, std::collections::TryReserveError> {
    PendingDiagnostic::new(
        DiagnosticKind::MetadataOmitted(failure),
        format,
        &[DiagnosticEffect::Metadata],
        None,
        "",
        "optional artifact metadata component could not be retained",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic_registry::LocalLimit;
    use crate::outcome::{DiagnosticAttribution, DiagnosticOrder};
    use crate::{DiagnosticCode, Recovery, ResourceValue};

    /// A retained diagnostic whose distinguishing state is its kind, pointer
    /// and the order the session assigned it when it was retained.
    fn retained(
        kind: DiagnosticKind<'_>,
        json_pointer: &str,
        owner_index: u64,
        source_position: u64,
        retention_ordinal: u64,
    ) -> CompatibilityDiagnostic {
        PendingDiagnostic::new(
            kind,
            OutputFormat::Svg,
            &[DiagnosticEffect::InputRepair],
            None,
            json_pointer,
            "",
        )
        .expect("the diagnostic allocates")
        .retained(DiagnosticOrder {
            owner_index,
            source_position,
            retention_ordinal,
            attribution: DiagnosticAttribution::SceneGlobal,
        })
    }

    /// `(pointer, retention ordinal)` per diagnostic: the pointer names the
    /// diagnostic and the ordinal tells which of two equal-keyed ones it was.
    fn order_of(diagnostics: &[CompatibilityDiagnostic]) -> Vec<(&str, u64)> {
        diagnostics
            .iter()
            .map(|diagnostic| {
                (
                    diagnostic.json_pointer(),
                    diagnostic.order().retention_ordinal,
                )
            })
            .collect()
    }

    /// The `resource.coordinate-range-exceeded` kind of an omitted outline.
    fn coordinate_crossing() -> DiagnosticKind<'static> {
        DiagnosticKind::LocalLimitCrossed {
            limit: LocalLimit::CoordinateMagnitude,
            configured: ResourceValue::Integer(1),
            attempted: crate::Measurement::Attempted {
                value: ResourceValue::Integer(2),
            },
            recovery: LocalRecovery::Omitted,
        }
    }

    /// Missing-member normalization positions must meet render-phase positions
    /// in the same fixed-width order on native and wasm32 (ADR-0002).
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
    #[cfg_attr(not(target_arch = "wasm32"), test)]
    fn missing_timestamp_sorts_after_render_phase_missing_glyph() {
        let scene = crate::SceneDocument::parse(
            r#"{"type":"excalidraw","version":2,"elements":[{"id":"text","type":"text","fontFamily":3,"text":"中"}],"appState":{},"files":{}}"#.as_bytes(),
        )
        .expect("valid scene");
        let request = crate::RenderRequest::builder(OutputFormat::Svg)
            .build()
            .expect("valid request");
        let outcome = crate::Renderer::new()
            .render(&scene, &request)
            .expect("rendered scene");
        let diagnostics = outcome.report().diagnostics();
        let actual = diagnostics
            .iter()
            .map(|diagnostic| (diagnostic.code().as_str(), diagnostic.json_pointer()))
            .collect::<Vec<_>>();
        assert_eq!(
            actual,
            [
                ("font.missing-glyph", "/elements/0"),
                ("normalization.missing-timestamp", "/elements/0/updated"),
            ]
        );
    }

    /// Spec §11.3: the final report is ordered by the source owner, not by
    /// the phase that reported. A render-phase coordinate crossing on the
    /// first element is retained after a normalization repair on the second
    /// and still sorts first.
    #[test]
    fn the_source_owner_outranks_the_phase_that_retained_a_diagnostic() {
        let mut diagnostics = vec![
            retained(
                DiagnosticKind::MissingTimestamp,
                "/elements/1/updated",
                1,
                7,
                0,
            ),
            retained(coordinate_crossing(), "/elements/0/x", 0, 2, 1),
        ];
        sort_diagnostics(&mut diagnostics);
        assert_eq!(
            order_of(&diagnostics),
            [("/elements/0/x", 1), ("/elements/1/updated", 0)]
        );
    }

    /// Within one owner the source member position decides, then the pointer,
    /// then the code's stable spelling, and finally the retention ordinal, so
    /// two diagnostics that agree on everything else keep the order in which
    /// they were retained.
    #[test]
    fn within_one_owner_position_then_pointer_then_code_then_retention_decide() {
        let unsupported_value = || DiagnosticKind::UnsupportedPropertyValue {
            recovery: Recovery::Substituted,
            recipe: None,
        };
        let mut diagnostics = vec![
            retained(DiagnosticKind::ZeroRoughSeed, "/elements/0/seed", 0, 9, 0),
            retained(
                DiagnosticKind::UnsupportedProperty,
                "/elements/0/b",
                0,
                4,
                1,
            ),
            retained(
                DiagnosticKind::UnsupportedProperty,
                "/elements/0/a",
                0,
                4,
                2,
            ),
            // Same pointer and position: "compatibility.unsupported-property"
            // sorts before "compatibility.unsupported-property-value".
            retained(unsupported_value(), "/elements/0/a", 0, 4, 3),
            retained(
                DiagnosticKind::UnsupportedProperty,
                "/elements/0/a",
                0,
                4,
                4,
            ),
        ];
        sort_diagnostics(&mut diagnostics);
        assert_eq!(
            order_of(&diagnostics),
            [
                ("/elements/0/a", 2),
                ("/elements/0/a", 4),
                ("/elements/0/a", 3),
                ("/elements/0/b", 1),
                ("/elements/0/seed", 0),
            ]
        );
    }

    /// A diagnostic with no source owner takes the unowned order, which sorts
    /// after every owned diagnostic; among themselves such scene-global
    /// diagnostics fall back to the pointer and then retention.
    #[test]
    fn diagnostics_without_a_source_owner_sort_after_every_owned_one() {
        let unowned = |pointer: &str, ordinal| {
            PendingDiagnostic::new(
                DiagnosticKind::MissingGlyph { codepoint: 0xfffd },
                OutputFormat::Svg,
                &[DiagnosticEffect::Visual],
                None,
                pointer,
                "",
            )
            .expect("the diagnostic allocates")
            .retained(DiagnosticOrder::retained(
                None,
                None,
                ordinal,
                DiagnosticAttribution::SceneGlobal,
            ))
        };
        let mut diagnostics = vec![
            unowned("", 0),
            retained(
                DiagnosticKind::MissingTimestamp,
                "/elements/3/updated",
                3,
                1,
                1,
            ),
            unowned("/appState/viewBackgroundColor", 2),
            unowned("", 3),
        ];
        sort_diagnostics(&mut diagnostics);
        assert_eq!(
            order_of(&diagnostics),
            [
                ("/elements/3/updated", 1),
                ("", 0),
                ("", 3),
                ("/appState/viewBackgroundColor", 2),
            ]
        );
    }

    /// Each codec's typed decode failure is one `image.decode-failed`
    /// diagnostic that names the media type the asset cache sniffed, with the
    /// format's recovery: the PNG placeholder, or omission for SVG and PDF.
    #[test]
    fn each_typed_decode_failure_names_its_sniffed_media_type() {
        let typed = [
            (asset::ResolveError::DecodeFailed, "image/png"),
            (asset::ResolveError::JpegDecodeFailed, "image/jpeg"),
            (asset::ResolveError::GifDecodeFailed, "image/gif"),
            (asset::ResolveError::WebPDecodeFailed, "image/webp"),
            (asset::ResolveError::BmpDecodeFailed, "image/bmp"),
            (asset::ResolveError::IcoDecodeFailed, "image/x-icon"),
            (asset::ResolveError::SvgDecodeFailed, "image/svg+xml"),
        ];
        for (failure, media_type) in typed {
            for format in [OutputFormat::Svg, OutputFormat::Png, OutputFormat::Pdf] {
                let diagnostic = image_asset_failure_diagnostic(
                    failure,
                    format,
                    "asset",
                    "image",
                    "/elements/0/fileId",
                    NormalizedImageStatus::Saved,
                    &ResourceLimits::default(),
                )
                .expect("the diagnostic allocates");

                assert_eq!(diagnostic.code(), DiagnosticCode::IMAGE_DECODE_FAILED);
                assert_eq!(diagnostic.effects(), [DiagnosticEffect::Visual]);
                assert_eq!(diagnostic.element_id(), Some("image"));
                assert_eq!(diagnostic.json_pointer(), "/elements/0/fileId");
                let Some(details) = diagnostic.details() else {
                    panic!("{failure:?} carries asset details");
                };
                let DiagnosticDetails::Asset {
                    asset_id,
                    claimed_media_type,
                    validated_media_type,
                    recovery,
                    recipe_id,
                } = details
                else {
                    panic!("{failure:?} carries asset details, not {details:?}");
                };
                assert_eq!(asset_id, "asset");
                assert_eq!(claimed_media_type.as_deref(), None);
                assert_eq!(validated_media_type.as_deref(), Some(media_type));
                match format {
                    OutputFormat::Png => {
                        assert_eq!(*recovery, Recovery::Placeholder);
                        assert!(recipe_id
                            .as_deref()
                            .is_some_and(|recipe| recipe.starts_with("official.image@sha256:")));
                    }
                    _ => {
                        assert_eq!(*recovery, Recovery::Omitted);
                        assert_eq!(recipe_id.as_deref(), None);
                    }
                }
            }
        }
    }

    /// A nested data image removed after local recovery keeps the code of the
    /// failure that removed it, with `nested_source` details in place of the
    /// asset or resource details a scene-level occurrence would carry.
    #[test]
    fn a_nested_resource_failure_keeps_its_measurement_and_source_details() {
        let failure = asset::svg::Failure {
            may_lose_semantics: true,
            placeholder: false,
            path: vec![(3, 1)],
            error: asset::ResolveError::SvgComplexity { attempted: 9 },
        };
        let diagnostic = nested_image_failure_diagnostic(
            &failure,
            OutputFormat::Pdf,
            "asset",
            "/files/asset/dataURL",
            &ResourceLimits::default(),
        )
        .expect("the diagnostic allocates");
        assert_eq!(
            diagnostic.code(),
            DiagnosticCode::RESOURCE_NESTED_SVG_COMPLEXITY_EXCEEDED
        );
        assert_eq!(diagnostic.element_id(), None);
        assert!(matches!(
            diagnostic.details(),
            Some(DiagnosticDetails::NestedResourceLimit {
                recovery: Recovery::Omitted,
                ..
            })
        ));
    }
}
