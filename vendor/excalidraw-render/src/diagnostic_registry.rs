//! The revision-1 compatibility diagnostic registry as a type.
//!
//! Spec section 11.3 binds every stable diagnostic code to the detail kind,
//! recovery and recipe it carries, and section 11.4 names the renderer-owned
//! fallback recipes. [`DiagnosticKind`] spells those bindings as variants:
//! each carries exactly the values its code's details need, so a stage that
//! diagnoses something names *what happened* and the registry derives the
//! code, the severity and the details. [`crate::outcome::PendingDiagnostic`]
//! is built only from a kind, which is what makes the bindings compiler
//! checked instead of a convention every construction site remembers.
//!
//! The one place a code is data rather than a variant is the sealed
//! scene-surface policy, whose field fallbacks name their code as a string;
//! [`DiagnosticKind::manifested`] admits only the bindings the registry has,
//! and the tests below check the generated table against it.

use std::collections::TryReserveError;

use crate::{
    AssetChainEntry, DiagnosticCode, DiagnosticDetails, DiagnosticSeverity, LogicalBounds,
    Measurement, MetadataComponent, MetadataOperation, NestedPath, OutputFormat, Recovery,
    ResourceValue, ValueKind,
};

/// A fallback recipe a diagnostic may name as its `recipe_id`: the three
/// renderer-owned recipes of spec section 11.4 and the manifested official
/// placeholders the revision-1 scene-surface manifest content-addresses.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FallbackRecipe {
    /// `Unsupported: <type>` box for an unknown element type.
    UnknownElement,
    /// `Malformed element` box.
    MalformedElement,
    /// `Unavailable: <type>` box for a known element this renderer cannot draw.
    KnownElementUnavailable,
    /// The official image placeholder PNG paints for an unavailable image.
    OfficialImage,
    /// The official image-error placeholder PNG paints for an image whose
    /// source status is `error`.
    OfficialImageError,
    /// The official placeholder for a rejected non-local image resource.
    OfficialForbiddenResource,
    /// The official offline embeddable label recipe.
    OfficialEmbeddableOffline,
}

impl FallbackRecipe {
    /// Every recipe, in the order the manifest and spec list them.
    pub(crate) const ALL: [Self; 7] = [
        Self::UnknownElement,
        Self::MalformedElement,
        Self::KnownElementUnavailable,
        Self::OfficialImage,
        Self::OfficialImageError,
        Self::OfficialForbiddenResource,
        Self::OfficialEmbeddableOffline,
    ];

    /// The stable `recipe_id` spelling.
    pub(crate) const fn id(self) -> &'static str {
        match self {
            Self::UnknownElement => "renderer.unknown-element.v1",
            Self::MalformedElement => "renderer.malformed-element.v1",
            Self::KnownElementUnavailable => "renderer.known-element-unavailable.v1",
            Self::OfficialImage => {
                "official.image@sha256:0a04dbba7ed6a0ce33aa61d9abf02b13685715241022483139b28eb666b8219d"
            }
            Self::OfficialImageError => {
                "official.image-error@sha256:f3c63c8600147961d006c0eea92ea6aff87bdc54711a2368af895a78154919e9"
            }
            Self::OfficialForbiddenResource => {
                "official.forbidden-resource@sha256:4452f99158233e3703e46dbbb83f6900a894fbceacd3e98b141332d552e22827"
            }
            Self::OfficialEmbeddableOffline => "official.embeddable-offline@sha256:d1d9207be2a4022886b9916ef0068b654c47f64d9ea59cd6d31d6432c5eecb01",
        }
    }

    /// The recipe with the stable spelling `id`, if the registry has one.
    pub(crate) fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|recipe| recipe.id() == id)
    }
}

/// How an occurrence that cannot be rendered is recovered locally: replaced by
/// a compatibility placeholder or omitted. The kind that carries it fixes
/// which recipe the placeholder is.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LocalRecovery {
    Placeholder,
    Omitted,
}

impl LocalRecovery {
    /// A placeholder when the occurrence can be replaced (it has safe finite
    /// bounds and fits its local limits), otherwise an omission.
    pub(crate) const fn placeholder_if(replaced: bool) -> Self {
        if replaced {
            Self::Placeholder
        } else {
            Self::Omitted
        }
    }

    pub(crate) const fn recovery(self) -> Recovery {
        match self {
            Self::Placeholder => Recovery::Placeholder,
            Self::Omitted => Recovery::Omitted,
        }
    }

    /// The recipe a placeholder paints; an omission names none.
    const fn recipe(self, placeholder: FallbackRecipe) -> Option<FallbackRecipe> {
        match self {
            Self::Placeholder => Some(placeholder),
            Self::Omitted => None,
        }
    }

    /// The local recovery a manifested `(recovery, recipe)` pair spells for a
    /// kind whose placeholder is `placeholder`, or `None` when the pair is not
    /// one the kind can carry.
    fn manifested(
        recovery: Recovery,
        recipe: Option<FallbackRecipe>,
        placeholder: FallbackRecipe,
    ) -> Option<Self> {
        match (recovery, recipe) {
            (Recovery::Placeholder, Some(recipe)) if recipe == placeholder => {
                Some(Self::Placeholder)
            }
            (Recovery::Omitted, None) => Some(Self::Omitted),
            _ => None,
        }
    }
}

/// The format recovery of an image occurrence whose payload cannot be drawn
/// (spec section 11.3, "format-specific omission/fallback"): PNG paints the
/// official image placeholder, or the image-error one when the referring
/// element's status is `error`; SVG and PDF omit the image.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ImageRecovery {
    OfficialImage,
    OfficialImageError,
    Omitted,
}

impl ImageRecovery {
    pub(crate) const fn for_format(format: OutputFormat, error_status: bool) -> Self {
        match format {
            OutputFormat::Png if error_status => Self::OfficialImageError,
            OutputFormat::Png => Self::OfficialImage,
            OutputFormat::Svg | OutputFormat::Pdf => Self::Omitted,
        }
    }

    const fn local(self) -> LocalRecovery {
        match self {
            Self::OfficialImage | Self::OfficialImageError => LocalRecovery::Placeholder,
            Self::Omitted => LocalRecovery::Omitted,
        }
    }

    const fn recipe(self) -> Option<FallbackRecipe> {
        match self {
            Self::OfficialImage => Some(FallbackRecipe::OfficialImage),
            Self::OfficialImageError => Some(FallbackRecipe::OfficialImageError),
            Self::Omitted => None,
        }
    }
}

/// A semantic resource ceiling whose crossing is recovered locally (spec
/// section 11.3, "Recoverable mappings"): each limit fixes the diagnostic
/// code, the `resource_limit` field and the value kind it is reported with.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LocalLimit {
    GeometrySegmentsPerElement,
    TextScalarsPerElement,
    ElementWorkUnits,
    AssetBytes,
    AssetWorkUnits,
    RasterDimension,
    DecodedPixelsPerAsset,
    NestedSvgNodes,
    NestedAssetDepth,
    CoordinateMagnitude,
}

impl LocalLimit {
    pub(crate) const fn code(self) -> DiagnosticCode {
        match self {
            Self::GeometrySegmentsPerElement | Self::TextScalarsPerElement => {
                DiagnosticCode::RESOURCE_ELEMENT_COMPLEXITY_REPLACED
            }
            Self::ElementWorkUnits => DiagnosticCode::RESOURCE_ELEMENT_WORK_EXCEEDED,
            Self::AssetBytes => DiagnosticCode::RESOURCE_ASSET_BYTES_EXCEEDED,
            Self::AssetWorkUnits => DiagnosticCode::RESOURCE_ASSET_WORK_EXCEEDED,
            Self::RasterDimension => DiagnosticCode::RESOURCE_IMAGE_DIMENSIONS_EXCEEDED,
            Self::DecodedPixelsPerAsset => DiagnosticCode::RESOURCE_IMAGE_PIXELS_EXCEEDED,
            Self::NestedSvgNodes => DiagnosticCode::RESOURCE_NESTED_SVG_COMPLEXITY_EXCEEDED,
            Self::NestedAssetDepth => DiagnosticCode::RESOURCE_NESTED_ASSET_DEPTH_EXCEEDED,
            Self::CoordinateMagnitude => DiagnosticCode::RESOURCE_COORDINATE_RANGE_EXCEEDED,
        }
    }

    /// The canonical `ResourceLimits` field name.
    pub(crate) const fn field(self) -> &'static str {
        match self {
            Self::GeometrySegmentsPerElement => "max_geometry_segments_per_element",
            Self::TextScalarsPerElement => "max_text_scalars_per_element",
            Self::ElementWorkUnits => "max_element_work_units",
            Self::AssetBytes => "max_asset_bytes",
            Self::AssetWorkUnits => "max_asset_work_units",
            Self::RasterDimension => "max_raster_dimension",
            Self::DecodedPixelsPerAsset => "max_decoded_pixels_per_asset",
            Self::NestedSvgNodes => "max_nested_svg_nodes",
            Self::NestedAssetDepth => "max_nested_asset_depth",
            Self::CoordinateMagnitude => "max_coordinate_magnitude",
        }
    }

    pub(crate) const fn value_kind(self) -> ValueKind {
        match self {
            Self::GeometrySegmentsPerElement
            | Self::TextScalarsPerElement
            | Self::NestedSvgNodes => ValueKind::Count,
            Self::DecodedPixelsPerAsset => ValueKind::Pixels,
            Self::ElementWorkUnits | Self::AssetWorkUnits => ValueKind::WorkUnits,
            Self::AssetBytes => ValueKind::Bytes,
            Self::RasterDimension => ValueKind::Dimension,
            Self::NestedAssetDepth => ValueKind::Depth,
            Self::CoordinateMagnitude => ValueKind::Coordinate,
        }
    }
}

/// Why a referenced image payload is unavailable to the target. Each variant
/// fixes the code; the kind that carries it decides whether the occurrence is
/// reported with `asset`, `resource_limit` or `nested_source` details.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum ImageFailure {
    ForbiddenResource,
    /// `image.missing-file`: the referring element has no authoritative payload.
    MissingFile,
    /// `asset.malformed-file-record`.
    MalformedFileRecord,
    /// `image.unsupported-format`.
    UnsupportedFormat,
    /// `image.decode-failed` for a payload the asset cache sniffed as
    /// `media_type`.
    DecodeFailed {
        media_type: &'static str,
    },
    /// A local asset ceiling crossed while resolving the payload.
    LimitCrossed {
        limit: LocalLimit,
        configured: ResourceValue,
        attempted: crate::ResourceMeasurement,
    },
}

impl ImageFailure {
    pub(crate) const fn code(self) -> DiagnosticCode {
        match self {
            Self::ForbiddenResource => DiagnosticCode::IMAGE_FORBIDDEN_RESOURCE,
            Self::MissingFile => DiagnosticCode::IMAGE_MISSING_FILE,
            Self::MalformedFileRecord => DiagnosticCode::ASSET_MALFORMED_FILE_RECORD,
            Self::UnsupportedFormat => DiagnosticCode::IMAGE_UNSUPPORTED_FORMAT,
            Self::DecodeFailed { .. } => DiagnosticCode::IMAGE_DECODE_FAILED,
            Self::LimitCrossed { limit, .. } => limit.code(),
        }
    }
}

/// An optional artifact-metadata operation that failed and was omitted. Each
/// variant fixes the `metadata.*` code and the component/operation pair spec
/// section 11.3 binds to it. A containing manifest operation preserves a more
/// specific component failure rather than relabeling it as manifest failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MetadataFailure {
    ProvenanceSerialization,
    DiagnosticSummarySerialization,
    SceneSerialization,
    SceneCompression,
    SceneEmbedding,
    ManifestSerialization,
    ManifestEmbedding,
}

impl MetadataFailure {
    pub(crate) const fn code(self) -> DiagnosticCode {
        match self {
            Self::ProvenanceSerialization => {
                DiagnosticCode::METADATA_PROVENANCE_SERIALIZATION_FAILED
            }
            Self::DiagnosticSummarySerialization => {
                DiagnosticCode::METADATA_DIAGNOSTIC_SUMMARY_SERIALIZATION_FAILED
            }
            Self::SceneSerialization => DiagnosticCode::METADATA_SCENE_SERIALIZATION_FAILED,
            Self::SceneCompression => DiagnosticCode::METADATA_SCENE_COMPRESSION_FAILED,
            Self::SceneEmbedding => DiagnosticCode::METADATA_SCENE_EMBEDDING_FAILED,
            Self::ManifestSerialization => DiagnosticCode::METADATA_MANIFEST_SERIALIZATION_FAILED,
            Self::ManifestEmbedding => DiagnosticCode::METADATA_MANIFEST_EMBEDDING_FAILED,
        }
    }

    pub(crate) const fn component(self) -> MetadataComponent {
        match self {
            Self::ProvenanceSerialization => MetadataComponent::Provenance,
            Self::DiagnosticSummarySerialization => MetadataComponent::DiagnosticSummary,
            Self::SceneSerialization | Self::SceneCompression | Self::SceneEmbedding => {
                MetadataComponent::EmbeddedScene
            }
            Self::ManifestSerialization | Self::ManifestEmbedding => MetadataComponent::Manifest,
        }
    }

    pub(crate) const fn operation(self) -> MetadataOperation {
        match self {
            Self::SceneCompression => MetadataOperation::Compression,
            Self::SceneEmbedding | Self::ManifestEmbedding => MetadataOperation::Embedding,
            Self::ProvenanceSerialization
            | Self::DiagnosticSummarySerialization
            | Self::SceneSerialization
            | Self::ManifestSerialization => MetadataOperation::Serialization,
        }
    }
}

/// What one compatibility diagnostic reports, carrying exactly the values its
/// code's details require. Borrowed strings are copied when the diagnostic is
/// built.
#[derive(Debug)]
pub(crate) enum DiagnosticKind<'a> {
    /// `normalization.missing-element-id`.
    MissingElementId,
    /// `normalization.duplicate-element-id`.
    DuplicateElementId,
    /// `normalization.missing-timestamp`.
    MissingTimestamp,
    /// `normalization.zero-rough-seed`.
    ZeroRoughSeed,
    /// `normalization.relationship-cycle`.
    RelationshipCycle,
    /// `compatibility.unsupported-element-type`, recovered with the
    /// `renderer.unknown-element.v1` box or omitted.
    UnsupportedElementType(LocalRecovery),
    /// `compatibility.malformed-element`, recovered with the
    /// `renderer.malformed-element.v1` box or omitted.
    MalformedElement(LocalRecovery),
    /// `compatibility.unsupported-property`.
    UnsupportedProperty,
    /// `compatibility.unsupported-property` for a property a later Excalidraw
    /// editor persists that its static export never reads: informational.
    EditorPropertyIgnored,
    /// `compatibility.unsupported-property-value`, with the recovery and
    /// recipe the manifest (or the normalizer's rule) declares for the field.
    UnsupportedPropertyValue {
        recovery: Recovery,
        recipe: Option<FallbackRecipe>,
    },
    /// `compatibility.known-element-replaced`, recovered with the
    /// `renderer.known-element-unavailable.v1` box or omitted.
    KnownElementReplaced(LocalRecovery),
    /// `asset.supplement-conflict`.
    SupplementConflict { asset_id: &'a str },
    /// `asset.media-type-mismatch`, with the claimed and validated types when
    /// each is known.
    MediaTypeMismatch {
        asset_id: &'a str,
        claimed_media_type: Option<&'a str>,
        validated_media_type: Option<&'a str>,
    },
    /// A referenced image whose payload cannot be drawn, reported with
    /// `asset` details, or `resource_limit` details when a local ceiling was
    /// crossed, and the format's image recovery.
    ImageUnavailable {
        failure: ImageFailure,
        asset_id: &'a str,
        recovery: ImageRecovery,
    },
    /// `image.forbidden-resource`, recovered with the official
    /// forbidden-resource placeholder or omitted.
    ForbiddenResource {
        asset_id: &'a str,
        recovery: LocalRecovery,
    },
    /// `image.animation-flattened` for a payload validated as `media_type`.
    AnimationFlattened {
        asset_id: &'a str,
        media_type: &'a str,
    },
    /// `image.unsafe-svg-content-removed`.
    UnsafeSvgContentRemoved {
        asset_chain: Vec<AssetChainEntry>,
        paths: Vec<NestedPath>,
    },
    NestedForbiddenResource {
        asset_chain: Vec<AssetChainEntry>,
        paths: Vec<NestedPath>,
        recovery: LocalRecovery,
    },
    /// A nested data URL's advisory media type repaired after validation.
    NestedMediaTypeMismatch {
        asset_chain: Vec<AssetChainEntry>,
        paths: Vec<NestedPath>,
        claimed_media_type: String,
        validated_media_type: String,
    },
    /// A nested data image removed after local recovery: the failure's code
    /// with `nested_source` details.
    NestedImageRemoved {
        failure: ImageFailure,
        asset_chain: Vec<AssetChainEntry>,
        paths: Vec<NestedPath>,
    },
    NestedResourceLimit {
        limit: LocalLimit,
        configured: ResourceValue,
        attempted: crate::ResourceMeasurement,
        asset_chain: Vec<AssetChainEntry>,
        paths: Vec<NestedPath>,
        recovery: LocalRecovery,
    },
    /// `link.unsafe-omitted`.
    UnsafeLinkOmitted,
    /// `color.invalid`.
    InvalidColor,
    /// `font.liberation-sans-substituted`.
    LiberationSansSubstituted,
    /// `font.noto-emoji-substituted`.
    NotoEmojiSubstituted,
    /// Noto Emoji supplied a glyph in an offline embeddable label.
    EmbeddableNotoEmojiSubstituted,
    /// `font.missing-glyph` for the Unicode scalar `codepoint`.
    MissingGlyph { codepoint: u32 },
    /// A local resource ceiling crossed by an element and recovered locally.
    LocalLimitCrossed {
        limit: LocalLimit,
        configured: ResourceValue,
        attempted: crate::ResourceMeasurement,
        recovery: LocalRecovery,
    },
    /// `resource.optional-metadata-omitted` after the crossing of `field`.
    OptionalMetadataOmitted {
        field: &'static str,
        configured: u64,
        value_kind: ValueKind,
        measurement: Measurement<u64>,
    },
    /// A `metadata.*` component failure whose component was omitted.
    MetadataOmitted(MetadataFailure),
    /// `pdf.raster-fallback` for an isolated closure that was rasterized. The
    /// registry binds every revision-1 code in every build, so the binding
    /// stays when the `pdf` feature that diagnoses it is off.
    #[cfg_attr(not(feature = "pdf"), allow(dead_code))]
    PdfRasterFallback {
        logical_bounds: LogicalBounds,
        pixel_width: u64,
        pixel_height: u64,
        causes: Vec<NestedPath>,
    },
}

impl DiagnosticKind<'_> {
    /// The kind a sealed field policy's fallback describes: `code` is the
    /// table's stable spelling, `recovery` its (format) recovery and `recipe`
    /// its recipe identity. `None` when the registry binds no kind to that
    /// spelling and shape; the registry tests check every generated fallback
    /// maps.
    pub(crate) fn manifested(code: &str, recovery: Recovery, recipe: Option<&str>) -> Option<Self> {
        let recipe = match recipe {
            Some(recipe) => Some(FallbackRecipe::from_id(recipe)?),
            None => None,
        };
        Some(match DiagnosticCode::from_stable_str(code)? {
            DiagnosticCode::COMPATIBILITY_UNSUPPORTED_PROPERTY_VALUE => {
                Self::UnsupportedPropertyValue { recovery, recipe }
            }
            DiagnosticCode::COLOR_INVALID
                if recovery == Recovery::Substituted && recipe.is_none() =>
            {
                Self::InvalidColor
            }
            DiagnosticCode::COMPATIBILITY_MALFORMED_ELEMENT => Self::MalformedElement(
                LocalRecovery::manifested(recovery, recipe, FallbackRecipe::MalformedElement)?,
            ),
            DiagnosticCode::COMPATIBILITY_UNSUPPORTED_ELEMENT_TYPE => Self::UnsupportedElementType(
                LocalRecovery::manifested(recovery, recipe, FallbackRecipe::UnknownElement)?,
            ),
            _ => return None,
        })
    }

    pub(crate) const fn code(&self) -> DiagnosticCode {
        match self {
            Self::NestedMediaTypeMismatch { .. } => DiagnosticCode::ASSET_MEDIA_TYPE_MISMATCH,
            Self::MissingElementId => DiagnosticCode::NORMALIZATION_MISSING_ELEMENT_ID,
            Self::DuplicateElementId => DiagnosticCode::NORMALIZATION_DUPLICATE_ELEMENT_ID,
            Self::MissingTimestamp => DiagnosticCode::NORMALIZATION_MISSING_TIMESTAMP,
            Self::ZeroRoughSeed => DiagnosticCode::NORMALIZATION_ZERO_ROUGH_SEED,
            Self::RelationshipCycle => DiagnosticCode::NORMALIZATION_RELATIONSHIP_CYCLE,
            Self::UnsupportedElementType(_) => {
                DiagnosticCode::COMPATIBILITY_UNSUPPORTED_ELEMENT_TYPE
            }
            Self::MalformedElement(_) => DiagnosticCode::COMPATIBILITY_MALFORMED_ELEMENT,
            Self::UnsupportedProperty | Self::EditorPropertyIgnored => {
                DiagnosticCode::COMPATIBILITY_UNSUPPORTED_PROPERTY
            }
            Self::UnsupportedPropertyValue { .. } => {
                DiagnosticCode::COMPATIBILITY_UNSUPPORTED_PROPERTY_VALUE
            }
            Self::KnownElementReplaced(_) => DiagnosticCode::COMPATIBILITY_KNOWN_ELEMENT_REPLACED,
            Self::SupplementConflict { .. } => DiagnosticCode::ASSET_SUPPLEMENT_CONFLICT,
            Self::MediaTypeMismatch { .. } => DiagnosticCode::ASSET_MEDIA_TYPE_MISMATCH,
            Self::ImageUnavailable { failure, .. } | Self::NestedImageRemoved { failure, .. } => {
                failure.code()
            }
            Self::NestedResourceLimit { limit, .. } => limit.code(),
            Self::ForbiddenResource { .. } | Self::NestedForbiddenResource { .. } => {
                DiagnosticCode::IMAGE_FORBIDDEN_RESOURCE
            }
            Self::AnimationFlattened { .. } => DiagnosticCode::IMAGE_ANIMATION_FLATTENED,
            Self::UnsafeSvgContentRemoved { .. } => {
                DiagnosticCode::IMAGE_UNSAFE_SVG_CONTENT_REMOVED
            }
            Self::UnsafeLinkOmitted => DiagnosticCode::LINK_UNSAFE_OMITTED,
            Self::InvalidColor => DiagnosticCode::COLOR_INVALID,
            Self::LiberationSansSubstituted => DiagnosticCode::FONT_LIBERATION_SANS_SUBSTITUTED,
            Self::NotoEmojiSubstituted | Self::EmbeddableNotoEmojiSubstituted => {
                DiagnosticCode::FONT_NOTO_EMOJI_SUBSTITUTED
            }
            Self::MissingGlyph { .. } => DiagnosticCode::FONT_MISSING_GLYPH,
            Self::LocalLimitCrossed { limit, .. } => limit.code(),
            Self::OptionalMetadataOmitted { .. } => {
                DiagnosticCode::RESOURCE_OPTIONAL_METADATA_OMITTED
            }
            Self::MetadataOmitted(failure) => failure.code(),
            Self::PdfRasterFallback { .. } => DiagnosticCode::PDF_RASTER_FALLBACK,
        }
    }

    /// Animation flattening, the two declared font substitutions, timestamp
    /// repair and ignored editor-only properties are informational (spec
    /// section 11.3).
    pub(crate) const fn severity(&self) -> DiagnosticSeverity {
        match self {
            Self::AnimationFlattened { .. }
            | Self::MissingTimestamp
            | Self::EditorPropertyIgnored
            | Self::LiberationSansSubstituted
            | Self::NotoEmojiSubstituted
            | Self::EmbeddableNotoEmojiSubstituted => DiagnosticSeverity::Info,
            _ => DiagnosticSeverity::Warning,
        }
    }

    /// The typed details the code requires.
    pub(crate) fn into_details(self) -> Result<DiagnosticDetails, TryReserveError> {
        Ok(match self {
            Self::NestedMediaTypeMismatch {
                asset_chain,
                paths,
                claimed_media_type,
                validated_media_type,
            } => DiagnosticDetails::NestedAssetClaim {
                asset_chain,
                paths,
                claimed_media_type,
                validated_media_type,
                recovery: Recovery::Repaired,
            },
            Self::MissingElementId
            | Self::DuplicateElementId
            | Self::MissingTimestamp
            | Self::ZeroRoughSeed => compatibility(Recovery::Repaired, None)?,
            Self::RelationshipCycle => compatibility(Recovery::Removed, None)?,
            Self::UnsupportedElementType(recovery) => compatibility(
                recovery.recovery(),
                recovery.recipe(FallbackRecipe::UnknownElement),
            )?,
            Self::MalformedElement(recovery) => compatibility(
                recovery.recovery(),
                recovery.recipe(FallbackRecipe::MalformedElement),
            )?,
            Self::UnsupportedProperty | Self::EditorPropertyIgnored => {
                compatibility(Recovery::Ignored, None)?
            }
            Self::UnsupportedPropertyValue { recovery, recipe } => compatibility(recovery, recipe)?,
            Self::KnownElementReplaced(recovery) => compatibility(
                recovery.recovery(),
                recovery.recipe(FallbackRecipe::KnownElementUnavailable),
            )?,
            Self::SupplementConflict { asset_id } => {
                asset(asset_id, None, None, Recovery::Ignored, None)?
            }
            Self::MediaTypeMismatch {
                asset_id,
                claimed_media_type,
                validated_media_type,
            } => asset(
                asset_id,
                claimed_media_type,
                validated_media_type,
                Recovery::Repaired,
                None,
            )?,
            Self::ImageUnavailable {
                failure:
                    ImageFailure::LimitCrossed {
                        limit,
                        configured,
                        attempted,
                    },
                recovery,
                ..
            } => resource_limit(limit, configured, attempted, recovery.local()),
            Self::ImageUnavailable {
                failure,
                asset_id,
                recovery,
            } => asset(
                asset_id,
                None,
                match failure {
                    ImageFailure::DecodeFailed { media_type } => Some(media_type),
                    ImageFailure::MissingFile
                    | ImageFailure::ForbiddenResource
                    | ImageFailure::MalformedFileRecord
                    | ImageFailure::UnsupportedFormat
                    | ImageFailure::LimitCrossed { .. } => None,
                },
                recovery.local().recovery(),
                recovery.recipe(),
            )?,
            Self::ForbiddenResource { asset_id, recovery } => asset(
                asset_id,
                None,
                None,
                recovery.recovery(),
                recovery.recipe(FallbackRecipe::OfficialForbiddenResource),
            )?,
            Self::AnimationFlattened {
                asset_id,
                media_type,
            } => asset(asset_id, None, Some(media_type), Recovery::Flattened, None)?,
            Self::UnsafeSvgContentRemoved { asset_chain, paths }
            | Self::NestedImageRemoved {
                asset_chain, paths, ..
            } => DiagnosticDetails::NestedSource {
                asset_chain,
                paths,
                recovery: Recovery::Removed,
            },
            Self::NestedForbiddenResource {
                asset_chain,
                paths,
                recovery,
            } => DiagnosticDetails::NestedSource {
                asset_chain,
                paths,
                recovery: recovery.recovery(),
            },
            Self::NestedResourceLimit {
                limit,
                configured,
                attempted,
                asset_chain,
                paths,
                recovery,
            } => DiagnosticDetails::NestedResourceLimit {
                field: limit.field(),
                configured,
                value_kind: limit.value_kind(),
                measurement: attempted,
                asset_chain,
                paths,
                recovery: recovery.recovery(),
                recipe_id: if recovery == LocalRecovery::Placeholder {
                    Some(try_owned("official.nested-resource@sha256:0fb00e2eda840ab048f9e0fc0698bb4b43d5618d6f67880149ce164412387677")?)
                } else {
                    None
                },
            },
            Self::UnsafeLinkOmitted => compatibility(Recovery::Omitted, None)?,
            Self::InvalidColor | Self::LiberationSansSubstituted | Self::NotoEmojiSubstituted => {
                compatibility(Recovery::Substituted, None)?
            }
            Self::EmbeddableNotoEmojiSubstituted => compatibility(
                Recovery::Substituted,
                Some(FallbackRecipe::OfficialEmbeddableOffline),
            )?,
            Self::MissingGlyph { codepoint } => DiagnosticDetails::MissingGlyph {
                codepoint,
                recovery: Recovery::Substituted,
            },
            Self::LocalLimitCrossed {
                limit,
                configured,
                attempted,
                recovery,
            } => resource_limit(limit, configured, attempted, recovery),
            Self::OptionalMetadataOmitted {
                field,
                configured,
                value_kind,
                measurement,
            } => DiagnosticDetails::ResourceLimit {
                field,
                configured: ResourceValue::Integer(configured),
                value_kind,
                measurement: match measurement {
                    Measurement::Attempted { value } => Measurement::Attempted {
                        value: ResourceValue::Integer(value),
                    },
                    Measurement::AtLeast { value } => Measurement::AtLeast {
                        value: ResourceValue::Integer(value),
                    },
                },
                recovery: Some(Recovery::MetadataOmitted),
            },
            Self::MetadataOmitted(failure) => DiagnosticDetails::Metadata {
                component: failure.component(),
                operation: failure.operation(),
                recovery: Recovery::MetadataOmitted,
            },
            Self::PdfRasterFallback {
                logical_bounds,
                pixel_width,
                pixel_height,
                causes,
            } => DiagnosticDetails::PdfRasterFallback {
                recovery: Some(Recovery::Rasterized),
                logical_bounds: Some(logical_bounds),
                pixel_width: Some(pixel_width),
                pixel_height: Some(pixel_height),
                causes,
            },
        })
    }
}

fn compatibility(
    recovery: Recovery,
    recipe: Option<FallbackRecipe>,
) -> Result<DiagnosticDetails, TryReserveError> {
    Ok(DiagnosticDetails::Compatibility {
        recovery,
        recipe_id: recipe.map(|recipe| try_owned(recipe.id())).transpose()?,
    })
}

fn asset(
    asset_id: &str,
    claimed_media_type: Option<&str>,
    validated_media_type: Option<&str>,
    recovery: Recovery,
    recipe: Option<FallbackRecipe>,
) -> Result<DiagnosticDetails, TryReserveError> {
    Ok(DiagnosticDetails::Asset {
        asset_id: try_owned(asset_id)?,
        claimed_media_type: claimed_media_type.map(try_owned).transpose()?,
        validated_media_type: validated_media_type.map(try_owned).transpose()?,
        recovery,
        recipe_id: recipe.map(|recipe| try_owned(recipe.id())).transpose()?,
    })
}

fn resource_limit(
    limit: LocalLimit,
    configured: ResourceValue,
    attempted: crate::ResourceMeasurement,
    recovery: LocalRecovery,
) -> DiagnosticDetails {
    DiagnosticDetails::ResourceLimit {
        field: limit.field(),
        configured,
        value_kind: limit.value_kind(),
        measurement: attempted,
        recovery: Some(recovery.recovery()),
    }
}

fn try_owned(value: &str) -> Result<String, TryReserveError> {
    let mut owned = String::new();
    owned.try_reserve_exact(value.len())?;
    owned.push_str(value);
    Ok(owned)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::Path;

    use super::*;
    use crate::scene_surface::SCENE_SURFACE_POLICY;

    /// The revision-1 scene-surface manifest the generated policy is
    /// imported from, parsed for the tables the generator does not emit.
    fn scene_surface_manifest() -> serde_json::Value {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/oracle/scene-surfaces/revision-1.json");
        let bytes = std::fs::read(&path).expect("the scene-surface manifest is checked in");
        serde_json::from_slice(&bytes).expect("the scene-surface manifest is JSON")
    }

    fn manifest_strings<'a>(
        manifest: &'a serde_json::Value,
        table: &str,
        member: &str,
    ) -> BTreeSet<&'a str> {
        manifest[table]
            .as_array()
            .unwrap_or_else(|| panic!("{table} is a manifest table"))
            .iter()
            .map(|entry| {
                entry[member]
                    .as_str()
                    .unwrap_or_else(|| panic!("{table}[].{member} is a string"))
            })
            .collect()
    }

    /// The manifest's registry mappings and the registered codes are the same
    /// set: a code the manifest binds that the renderer cannot spell, or a
    /// registered code the manifest does not know, fails here.
    #[test]
    fn registered_codes_are_exactly_the_manifest_registry() {
        let manifest = scene_surface_manifest();
        let manifested = manifest_strings(&manifest, "registryMappings", "code");
        let registered = DiagnosticCode::ALL
            .iter()
            .map(|code| code.as_str())
            .collect::<BTreeSet<_>>();
        assert_eq!(registered.len(), DiagnosticCode::ALL.len());
        assert_eq!(registered, manifested);
        for code in DiagnosticCode::ALL {
            assert_eq!(DiagnosticCode::from_stable_str(code.as_str()), Some(code));
        }
        assert_eq!(
            DiagnosticCode::from_stable_str("scene.invalid-envelope"),
            None
        );
    }

    /// Every code the generated field policies name is registered, except
    /// the scene envelope members, whose invalidity is the fatal
    /// `scene.invalid-envelope` parse error rather than a diagnostic. Every
    /// fallback whose code carries `compatibility` details maps to a registry
    /// kind for each recovery it declares, so the surface classifier never
    /// drops a manifested recovery; the three fallbacks whose codes carry
    /// `asset` details are ones the classifier reports through its own
    /// asset-shaped constructors (`/mimeType`, and the `/$key` pseudo-field,
    /// which describes the file record's map key and is never a member path,
    /// #156) or never walks (`nested.local_svg_resource`).
    #[test]
    fn generated_field_fallbacks_map_to_registry_kinds() {
        const ASSET_SHAPED: [(&str, &str, &str); 3] = [
            ("file_record", "/$key", "asset.malformed-file-record"),
            ("file_record", "/mimeType", "asset.media-type-mismatch"),
            (
                "nested.local_svg_resource",
                "/*",
                "image.forbidden-resource",
            ),
        ];
        let mut unmapped = Vec::new();
        for field in SCENE_SURFACE_POLICY.fields {
            let Some(code) = field.invalid.code else {
                continue;
            };
            if code == "scene.invalid-envelope" {
                assert_eq!(field.record_kind, "scene", "{}", field.path);
                assert!(field.invalid.recovery.is_none(), "{}", field.path);
                continue;
            }
            assert!(
                DiagnosticCode::from_stable_str(code).is_some(),
                "{} {} names unregistered code {code}",
                field.record_kind,
                field.path
            );
            let recoveries = field
                .invalid
                .recovery
                .into_iter()
                .chain(field.invalid.format_recoveries.into_iter().flatten());
            for recovery in recoveries {
                let recovery = Recovery::from_manifest(recovery)
                    .unwrap_or_else(|| panic!("{} names recovery {recovery}", field.path));
                let kind = DiagnosticKind::manifested(code, recovery, field.invalid.recipe);
                match kind {
                    Some(kind) => assert_eq!(kind.code().as_str(), code, "{}", field.path),
                    None => unmapped.push((field.record_kind, field.path, code)),
                }
            }
        }
        assert_eq!(unmapped, ASSET_SHAPED);
    }

    /// The recipes the registry can name are exactly the manifest's: the
    /// renderer-owned references and the official recipes the asset catalog
    /// seals, except nested-resource, which this typed subset does not name.
    #[test]
    fn recipe_identities_are_the_manifests() {
        let manifest = scene_surface_manifest();
        let renderer_owned =
            manifest_strings(&manifest, "rendererOwnedRecipeReferences", "recipeId");
        let official = manifest_strings(&manifest, "fallbackRecipes", "recipeId");
        assert_eq!(
            official,
            crate::fonts::revision_1()
                .fallback_recipe_identities
                .iter()
                .copied()
                .collect::<BTreeSet<_>>()
        );
        for recipe in FallbackRecipe::ALL {
            assert_eq!(FallbackRecipe::from_id(recipe.id()), Some(recipe));
            let known = if recipe.id().starts_with("renderer.") {
                renderer_owned.contains(recipe.id())
            } else {
                official.contains(recipe.id())
            };
            assert!(known, "{}", recipe.id());
        }
        assert_eq!(
            renderer_owned.len(),
            FallbackRecipe::ALL
                .iter()
                .filter(|recipe| recipe.id().starts_with("renderer."))
                .count()
        );
        for field in SCENE_SURFACE_POLICY.fields {
            if let Some(recipe) = field.invalid.recipe {
                assert!(FallbackRecipe::from_id(recipe).is_some(), "{}", field.path);
            }
        }
    }

    /// Each detail-bearing variant's details agree with spec section 11.3:
    /// the detail kind, the recovery it fixes, and the recipe its placeholder
    /// names.
    #[test]
    fn details_follow_the_registry_bindings() {
        let details = |kind: DiagnosticKind<'_>| kind.into_details().expect("details allocate");
        assert_eq!(
            details(DiagnosticKind::MissingElementId),
            DiagnosticDetails::Compatibility {
                recovery: Recovery::Repaired,
                recipe_id: None,
            }
        );
        assert_eq!(
            details(DiagnosticKind::RelationshipCycle).recovery(),
            Some(Recovery::Removed)
        );
        assert_eq!(
            details(DiagnosticKind::KnownElementReplaced(
                LocalRecovery::Placeholder
            )),
            DiagnosticDetails::Compatibility {
                recovery: Recovery::Placeholder,
                recipe_id: Some(FallbackRecipe::KnownElementUnavailable.id().to_owned()),
            }
        );
        assert_eq!(
            details(DiagnosticKind::UnsupportedElementType(
                LocalRecovery::Omitted
            )),
            DiagnosticDetails::Compatibility {
                recovery: Recovery::Omitted,
                recipe_id: None,
            }
        );
        assert_eq!(
            details(DiagnosticKind::ImageUnavailable {
                failure: ImageFailure::DecodeFailed {
                    media_type: "image/webp",
                },
                asset_id: "asset",
                recovery: ImageRecovery::for_format(OutputFormat::Png, true),
            }),
            DiagnosticDetails::Asset {
                asset_id: "asset".to_owned(),
                claimed_media_type: None,
                validated_media_type: Some("image/webp".to_owned()),
                recovery: Recovery::Placeholder,
                recipe_id: Some(FallbackRecipe::OfficialImageError.id().to_owned()),
            }
        );
        assert_eq!(
            details(DiagnosticKind::ImageUnavailable {
                failure: ImageFailure::LimitCrossed {
                    limit: LocalLimit::AssetBytes,
                    configured: ResourceValue::Integer(10),
                    attempted: Measurement::Attempted {
                        value: ResourceValue::Integer(11)
                    },
                },
                asset_id: "asset",
                recovery: ImageRecovery::for_format(OutputFormat::Svg, true),
            }),
            DiagnosticDetails::ResourceLimit {
                field: "max_asset_bytes",
                configured: ResourceValue::Integer(10),
                value_kind: ValueKind::Bytes,
                measurement: Measurement::Attempted {
                    value: ResourceValue::Integer(11),
                },
                recovery: Some(Recovery::Omitted),
            }
        );
        assert_eq!(
            details(DiagnosticKind::MetadataOmitted(
                MetadataFailure::SceneCompression
            )),
            DiagnosticDetails::Metadata {
                component: MetadataComponent::EmbeddedScene,
                operation: MetadataOperation::Compression,
                recovery: Recovery::MetadataOmitted,
            }
        );
        assert_eq!(
            details(DiagnosticKind::MissingGlyph { codepoint: 0x1F600 }).recovery(),
            Some(Recovery::Substituted)
        );
    }

    /// The image recovery rule of spec section 11.3: PNG paints the official
    /// placeholder, the error one for an `error` status; SVG and PDF omit.
    #[test]
    fn image_recovery_follows_the_format() {
        assert_eq!(
            ImageRecovery::for_format(OutputFormat::Png, false),
            ImageRecovery::OfficialImage
        );
        assert_eq!(
            ImageRecovery::for_format(OutputFormat::Png, true),
            ImageRecovery::OfficialImageError
        );
        for format in [OutputFormat::Svg, OutputFormat::Pdf] {
            assert_eq!(
                ImageRecovery::for_format(format, true),
                ImageRecovery::Omitted
            );
            assert_eq!(
                ImageRecovery::for_format(format, false),
                ImageRecovery::Omitted
            );
        }
    }

    /// `manifested` admits exactly the bindings the registry has: an unknown
    /// code, an unregistered recipe, or a recovery/recipe pair a kind cannot
    /// carry yields no kind.
    #[test]
    fn manifested_admits_only_registered_bindings() {
        assert!(matches!(
            DiagnosticKind::manifested(
                "compatibility.unsupported-property-value",
                Recovery::Placeholder,
                Some(FallbackRecipe::KnownElementUnavailable.id()),
            ),
            Some(DiagnosticKind::UnsupportedPropertyValue {
                recovery: Recovery::Placeholder,
                recipe: Some(FallbackRecipe::KnownElementUnavailable),
            })
        ));
        assert!(matches!(
            DiagnosticKind::manifested("color.invalid", Recovery::Substituted, None),
            Some(DiagnosticKind::InvalidColor)
        ));
        assert!(DiagnosticKind::manifested("color.invalid", Recovery::Omitted, None).is_none());
        assert!(matches!(
            DiagnosticKind::manifested(
                "compatibility.malformed-element",
                Recovery::Placeholder,
                Some(FallbackRecipe::MalformedElement.id()),
            ),
            Some(DiagnosticKind::MalformedElement(LocalRecovery::Placeholder))
        ));
        assert!(DiagnosticKind::manifested(
            "compatibility.malformed-element",
            Recovery::Placeholder,
            Some(FallbackRecipe::UnknownElement.id()),
        )
        .is_none());
        assert!(
            DiagnosticKind::manifested("scene.invalid-envelope", Recovery::Omitted, None).is_none()
        );
        assert!(
            DiagnosticKind::manifested("font.missing-glyph", Recovery::Substituted, None).is_none()
        );
        assert!(DiagnosticKind::manifested(
            "compatibility.unsupported-property-value",
            Recovery::Placeholder,
            Some("renderer.future.v2"),
        )
        .is_none());
    }
}
