use std::fmt;

use serde::ser::SerializeStruct;
use serde::Serialize;

use crate::outcome::{RenderError, RenderErrorCategory, RenderOutcome};
use crate::request::{OutputFormat, RenderRequest};
use crate::scene::SceneDocument;

// Hidden, with the accessors that return the identity: this release implements
// a single revision, so there is nothing to choose between, and the identity
// joins the supported API when a second revision exists. The CLI prints it for
// `--version --verbose`.
/// Identifies the static-export profile a renderer release implements.
///
/// This release implements exactly one revision, [`REVISION_1`]. During 0.x a
/// revision may change in any release and no historical revision is
/// retained; pin the package version and dependency lockfile when historical
/// output bytes are required.
///
/// [`REVISION_1`]: CompatibilityRevision::REVISION_1
#[doc(hidden)]
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct CompatibilityRevision(u64);

impl CompatibilityRevision {
    /// Compatibility revision 1, bound to Excalidraw v0.18.1.
    pub const REVISION_1: Self = Self(1);

    /// Returns the numeric compatibility revision.
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for CompatibilityRevision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Immutable identity of the reference profile a renderer reproduces.
// Hidden, like `CompatibilityRevision`.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize)]
pub struct ReferenceProfileIdentity {
    compatibility_revision: CompatibilityRevision,
    excalidraw_release: &'static str,
    reference_profile_digest: &'static str,
}

impl ReferenceProfileIdentity {
    pub(crate) const fn new(
        compatibility_revision: CompatibilityRevision,
        excalidraw_release: &'static str,
        reference_profile_digest: &'static str,
    ) -> Self {
        Self {
            compatibility_revision,
            excalidraw_release,
            reference_profile_digest,
        }
    }

    /// Returns the compatibility revision bound to this reference profile.
    pub const fn compatibility_revision(&self) -> CompatibilityRevision {
        self.compatibility_revision
    }

    /// Returns the pinned official Excalidraw release.
    pub const fn excalidraw_release(&self) -> &'static str {
        self.excalidraw_release
    }

    /// Returns the canonical SHA-256 digest of the reference profile.
    pub const fn reference_profile_digest(&self) -> &'static str {
        self.reference_profile_digest
    }
}

/// Immutable renderer for this release's compatibility revision.
///
/// A renderer holds no per-render state: it is cheap to construct, `Clone`,
/// `Send` and `Sync`, and one instance can serve concurrent renders.
#[derive(Clone, Debug)]
pub struct Renderer {
    catalog: &'static crate::catalog::RevisionCatalog,
}

impl Renderer {
    /// Constructs a renderer.
    pub fn new() -> Self {
        Self {
            catalog: &crate::catalog::REVISION_1,
        }
    }

    /// Returns the reference profile scene exports reproduce.
    #[doc(hidden)]
    pub const fn profile_identity(&self) -> &'static ReferenceProfileIdentity {
        self.catalog.profile()
    }

    /// Returns the reference profile frame exports reproduce. It shares the
    /// scene profile's revision and release but has its own digest.
    #[doc(hidden)]
    pub const fn frame_profile_identity(&self) -> &'static ReferenceProfileIdentity {
        self.catalog.frame_profile()
    }

    /// Renders one admitted scene into the request's selected output format.
    ///
    /// # Errors
    ///
    /// Returns a [`RenderError`] when no complete artifact can be produced.
    /// Its [`category`](RenderError::category) says why:
    ///
    /// - [`Input`](RenderErrorCategory::Input): the scene has no finite
    ///   bounds (`scene.unbounded-geometry`).
    /// - [`Capability`](RenderErrorCategory::Capability): a font the scene
    ///   needs is not in this build (`render.font-unavailable`; see the
    ///   crate's `cjk` feature).
    /// - [`ResourceLimit`](RenderErrorCategory::ResourceLimit): the render
    ///   crosses one of the request's [`ResourceLimits`](crate::ResourceLimits)
    ///   (a `resource.*` code), or a required allocation failed
    ///   (`resource.memory-unavailable`).
    /// - [`NoMeaningfulArtifact`](RenderErrorCategory::NoMeaningfulArtifact):
    ///   a PDF raster fallback was forbidden by the request or could not be
    ///   isolated (`pdf.raster-fallback-*`).
    /// - [`Encoding`](RenderErrorCategory::Encoding): writing the artifact
    ///   failed (an `encode.*` code).
    /// - [`Internal`](RenderErrorCategory::Internal): a renderer invariant
    ///   failed (`render.internal-failure`).
    ///
    /// The error carries the complete [report](RenderError::report)
    /// accumulated before the failure.
    pub fn render(
        &self,
        scene: &SceneDocument,
        request: &RenderRequest,
    ) -> Result<RenderOutcome, RenderError> {
        crate::render::render(self.catalog, scene, request)
    }

    /// Renders one discovered frame using reference frame-export semantics.
    ///
    /// # Errors
    ///
    /// Returns a [`RenderError`] in the same categories as
    /// [`render`](Self::render). A frame whose viewport falls outside
    /// `max_coordinate_magnitude` fails with
    /// `resource.coordinate-range-exceeded`.
    pub fn render_frame(
        &self,
        frame: crate::FrameRef<'_>,
        request: &crate::FrameRenderRequest,
    ) -> Result<RenderOutcome, RenderError> {
        crate::render::render_frame(self.catalog, frame, request)
    }

    /// Starts lazy source-ordered rendering for selected frames.
    ///
    /// # Errors
    ///
    /// Returns a [`FrameBatchError`](crate::FrameBatchError) before rendering
    /// anything when a selected frame belongs to another scene document
    /// (`frame-batch.foreign-target`), when the same frame is selected twice
    /// (`frame-batch.duplicate-target`), or when the preparation the frames
    /// share cannot be completed (`frame-batch.shared-preparation-failed`).
    /// Each frame's own render failure is reported by its
    /// [`FrameRenderEntry`](crate::FrameRenderEntry) instead.
    pub fn render_frames<'scene, 'request>(
        &self,
        scene: &'scene SceneDocument,
        frames: impl IntoIterator<Item = crate::FrameRef<'scene>>,
        request: &'request crate::FrameRenderRequest,
    ) -> Result<crate::FrameRenderCursor<'scene, 'request>, crate::FrameBatchError> {
        crate::FrameRenderCursor::new(self.catalog, scene, frames, request)
    }
}

impl Default for Renderer {
    fn default() -> Self {
        Self::new()
    }
}

/// Stable family of a typed renderer failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum ErrorFamily {
    /// Scene-document bytes could not be admitted.
    SceneParse,
    /// A render request is invalid.
    Request,
    /// Rendering failed after request validation.
    Render,
    /// A lazy frame batch could not be constructed.
    FrameBatch,
}

impl ErrorFamily {
    /// Returns the stable serialized spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SceneParse => "scene_parse",
            Self::Request => "request",
            Self::Render => "render",
            Self::FrameBatch => "frame_batch",
        }
    }
}

impl fmt::Display for ErrorFamily {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// String-backed stable error code.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct ErrorCode(&'static str);

impl ErrorCode {
    /// Request padding is negative or non-finite.
    pub const REQUEST_INVALID_PADDING: Self = Self("request.invalid-padding");

    /// Request scale is non-positive or non-finite.
    pub const REQUEST_INVALID_SCALE: Self = Self("request.invalid-scale");

    /// Request maximum dimension is non-positive or non-finite.
    pub const REQUEST_INVALID_MAX_DIMENSION: Self = Self("request.invalid-max-dimension");

    /// Format-specific options were supplied for another output format.
    pub const REQUEST_INCOMPATIBLE_FORMAT_OPTIONS: Self =
        Self("request.incompatible-format-options");

    /// A supplemental asset has an empty scene file ID.
    pub const REQUEST_EMPTY_ASSET_ID: Self = Self("request.empty-asset-id");

    /// A supplemental scene file ID occurs more than once.
    pub const REQUEST_DUPLICATE_ASSET_ID: Self = Self("request.duplicate-asset-id");

    /// A supplemental asset advisory media type is malformed.
    pub const REQUEST_INVALID_ASSET_MEDIA_TYPE: Self = Self("request.invalid-asset-media-type");

    /// A semantic resource ceiling is zero, invalid, inconsistent, or too large.
    pub const REQUEST_INVALID_RESOURCE_LIMIT: Self = Self("request.invalid-resource-limit");

    /// The requested output format's Cargo feature (`png` or `pdf`) is not
    /// enabled in this build.
    pub const REQUEST_FORMAT_UNAVAILABLE: Self = Self("request.format-unavailable");

    /// Finite whole-scene geometry could not be established.
    pub const SCENE_UNBOUNDED_GEOMETRY: Self = Self("scene.unbounded-geometry");
    /// A required PDF raster fallback was forbidden by the request.
    pub const PDF_RASTER_FALLBACK_FORBIDDEN: Self = Self("pdf.raster-fallback-forbidden");
    /// A PDF raster fallback could not be isolated safely.
    pub const PDF_RASTER_FALLBACK_UNISOLATABLE: Self = Self("pdf.raster-fallback-unisolatable");
    /// No meaningful complete artifact could be produced.
    pub const RENDER_NO_MEANINGFUL_ARTIFACT: Self = Self("render.no-meaningful-artifact");
    /// A demanded bundled font is unavailable in this build.
    pub const RENDER_FONT_UNAVAILABLE: Self = Self("render.font-unavailable");
    /// SVG encoding failed.
    pub const ENCODE_SVG_FAILED: Self = Self("encode.svg-failed");
    /// A required direct-SVG font subset could not be produced or validated.
    pub const ENCODE_SVG_FONT_SUBSET_FAILED: Self = Self("encode.svg-font-subset-failed");
    /// PNG encoding failed.
    pub const ENCODE_PNG_FAILED: Self = Self("encode.png-failed");
    /// PDF encoding failed.
    pub const ENCODE_PDF_FAILED: Self = Self("encode.pdf-failed");
    /// An internal renderer invariant failed.
    pub const RENDER_INTERNAL_FAILURE: Self = Self("render.internal-failure");
    /// Admitted scene bytes exceed the configured semantic ceiling.
    pub const RESOURCE_SCENE_BYTES_EXCEEDED: Self = Self("resource.scene-bytes-exceeded");
    /// Source element occurrences exceed the configured ceiling.
    pub const RESOURCE_ELEMENT_COUNT_EXCEEDED: Self = Self("resource.element-count-exceeded");
    /// Asset payload occurrences exceed the configured ceiling.
    pub const RESOURCE_ASSET_COUNT_EXCEEDED: Self = Self("resource.asset-count-exceeded");
    /// Aggregate asset bytes exceed the configured ceiling.
    pub const RESOURCE_TOTAL_ASSET_BYTES_EXCEEDED: Self =
        Self("resource.total-asset-bytes-exceeded");
    /// Relationship traversal exceeds the configured depth.
    pub const RESOURCE_RELATIONSHIP_DEPTH_EXCEEDED: Self =
        Self("resource.relationship-depth-exceeded");
    /// Aggregate geometry exceeds the configured segment ceiling.
    pub const RESOURCE_TOTAL_GEOMETRY_EXCEEDED: Self = Self("resource.total-geometry-exceeded");
    /// Aggregate text exceeds the configured scalar ceiling.
    pub const RESOURCE_TOTAL_TEXT_EXCEEDED: Self = Self("resource.total-text-exceeded");
    /// Aggregate decoded pixels exceed the configured ceiling.
    pub const RESOURCE_TOTAL_DECODED_PIXELS_EXCEEDED: Self =
        Self("resource.total-decoded-pixels-exceeded");
    /// The required main raster surface exceeds a side or area ceiling.
    pub const RESOURCE_RASTER_SURFACE_EXCEEDED: Self = Self("resource.raster-surface-exceeded");
    /// Aggregate fallback rasterization exceeds the configured pixel ceiling.
    pub const RESOURCE_TOTAL_RASTERIZED_PIXELS_EXCEEDED: Self =
        Self("resource.total-rasterized-pixels-exceeded");
    /// Logical or scaled SVG dimensions exceed the configured ceiling.
    pub const RESOURCE_SVG_DIMENSIONS_EXCEEDED: Self = Self("resource.svg-dimensions-exceeded");
    /// PDF page dimensions exceed the configured point ceiling.
    pub const RESOURCE_PDF_PAGE_DIMENSIONS_EXCEEDED: Self =
        Self("resource.pdf-page-dimensions-exceeded");
    /// Diagnostic occurrences exceed the configured ceiling.
    pub const RESOURCE_DIAGNOSTIC_COUNT_EXCEEDED: Self = Self("resource.diagnostic-count-exceeded");
    /// Canonical logical working bytes exceed the configured ceiling.
    pub const RESOURCE_WORKING_BYTES_EXCEEDED: Self = Self("resource.working-bytes-exceeded");
    /// Complete artifact bytes exceed the configured ceiling.
    pub const RESOURCE_OUTPUT_BYTES_EXCEEDED: Self = Self("resource.output-bytes-exceeded");
    /// Aggregate semantic work exceeds the configured ceiling.
    pub const RESOURCE_TOTAL_WORK_EXCEEDED: Self = Self("resource.total-work-exceeded");
    /// A selected frame has no viewport within the coordinate ceiling.
    pub const RESOURCE_COORDINATE_RANGE_EXCEEDED: Self = Self("resource.coordinate-range-exceeded");
    /// A required physical allocation was unavailable.
    pub const RESOURCE_MEMORY_UNAVAILABLE: Self = Self("resource.memory-unavailable");

    /// A selected frame belongs to another scene document.
    pub const FRAME_BATCH_FOREIGN_TARGET: Self = Self("frame-batch.foreign-target");

    /// The same frame occurrence was selected more than once.
    pub const FRAME_BATCH_DUPLICATE_TARGET: Self = Self("frame-batch.duplicate-target");

    /// Shared target-neutral frame preparation could not be completed.
    pub const FRAME_BATCH_SHARED_PREPARATION_FAILED: Self =
        Self("frame-batch.shared-preparation-failed");

    /// The scene-document bytes are not valid UTF-8.
    pub const SCENE_INVALID_UTF8: Self = Self("scene.invalid-utf8");

    /// The scene-document bytes do not contain valid JSON syntax.
    pub const SCENE_INVALID_JSON: Self = Self("scene.invalid-json");

    /// The scene envelope does not have the required version-2 shape.
    pub const SCENE_INVALID_ENVELOPE: Self = Self("scene.invalid-envelope");

    /// The input is a recognized file that is not a scene document: a PNG,
    /// SVG or PDF export, or an Excalidraw library or clipboard payload.
    pub const SCENE_UNSUPPORTED_INPUT: Self = Self("scene.unsupported-input");

    /// The scene-document bytes exceed the fixed parser input ceiling.
    pub const SCENE_INPUT_TOO_LARGE: Self = Self("scene.input-too-large");

    /// The JSON nesting exceeds the fixed parser depth ceiling.
    pub const SCENE_JSON_DEPTH_EXCEEDED: Self = Self("scene.json-depth-exceeded");

    /// The JSON structure exceeds the fixed parser entry ceiling.
    pub const SCENE_JSON_STRUCTURE_EXCEEDED: Self = Self("scene.json-structure-exceeded");

    /// A decoded string or the decoded-string aggregate exceeds its ceiling.
    pub const SCENE_STRING_TOO_LARGE: Self = Self("scene.string-too-large");

    /// An object key exceeds the fixed parser key ceiling.
    pub const SCENE_OBJECT_KEY_TOO_LARGE: Self = Self("scene.object-key-too-large");

    /// A number token exceeds the fixed parser token ceiling.
    pub const SCENE_NUMBER_TOKEN_TOO_LARGE: Self = Self("scene.number-token-too-large");

    /// A later object member duplicates an earlier decoded key.
    pub const SCENE_DUPLICATE_OBJECT_KEY: Self = Self("scene.duplicate-object-key");

    /// Canonical parser live bytes exceed the fixed parser memory ceiling.
    pub const SCENE_PARSE_MEMORY_EXCEEDED: Self = Self("scene.parse-memory-exceeded");

    /// Returns the stable serialized spelling.
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Typed details attached to an [`ErrorDescriptor`] or a
/// [`CompatibilityDiagnostic`](crate::CompatibilityDiagnostic).
///
/// Errors and diagnostics share one detail vocabulary; each code fixes
/// which variant, if any, it carries.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[non_exhaustive]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DiagnosticDetails {
    /// Identifies a fixed scene-parser ceiling crossing.
    #[non_exhaustive]
    ParseLimit {
        /// The canonical parser-limit field.
        field: &'static str,
        /// The fixed configured ceiling.
        configured: u64,
        /// The unit measured by this ceiling.
        value_kind: ValueKind,
        /// The measured attempted or lower-bound value.
        measurement: Measurement,
    },
    /// Identifies the rejected request field and any applicable asset or option.
    #[non_exhaustive]
    RequestValue {
        /// The canonical request field.
        field: &'static str,
        /// The applicable supplemental asset ID.
        asset_id: Option<String>,
        /// The applicable format-option name.
        option: Option<&'static str>,
    },
    /// Describes a compatibility recovery and optional renderer recipe.
    #[non_exhaustive]
    Compatibility {
        /// The recovery applied to retain a meaningful artifact.
        recovery: Recovery,
        /// The stable renderer recipe identity, when applicable.
        recipe_id: Option<String>,
    },
    /// Describes an authoritative or supplemental asset recovery.
    #[non_exhaustive]
    Asset {
        /// Scene file ID of the affected asset.
        asset_id: String,
        /// Caller-claimed advisory media type.
        claimed_media_type: Option<String>,
        /// Media type established from the payload bytes.
        validated_media_type: Option<String>,
        /// Recovery applied to the asset occurrence.
        recovery: Recovery,
        /// Stable renderer recipe identity, when applicable.
        recipe_id: Option<String>,
    },
    /// Describes a fatal or recoverable semantic resource crossing.
    #[non_exhaustive]
    ResourceLimit {
        /// Canonical resource-limit field.
        field: &'static str,
        /// Configured semantic ceiling.
        configured: ResourceValue,
        /// Unit measured by the ceiling.
        value_kind: ValueKind,
        /// Measured attempted or lower-bound value.
        measurement: ResourceMeasurement,
        /// Recovery applied for a local crossing; null for a fatal crossing.
        recovery: Option<Recovery>,
    },
    /// Identifies a physical allocation that could not be satisfied.
    #[non_exhaustive]
    MemoryUnavailable {
        /// Stable category of allocation subject.
        subject_kind: String,
        /// Stable identity of the allocation subject.
        subject: String,
    },
    /// Identifies one missing Unicode scalar and its substitution.
    #[non_exhaustive]
    MissingGlyph {
        /// Missing Unicode scalar value.
        codepoint: u32,
        /// Required substitution recovery.
        recovery: Recovery,
    },
    /// Identifies a demanded bundled font unavailable in this build.
    #[non_exhaustive]
    FontUnavailable {
        /// Resolved bundled family required by the render.
        resolved_family: String,
        /// Official source chunk identity.
        asset_identity: String,
    },
    /// Identifies a fatal direct-SVG font subset operation.
    #[non_exhaustive]
    SvgFontSubsetFailure {
        /// First failed subset pipeline stage.
        stage: SvgFontSubsetStage,
        /// Resolved bundled family being derived.
        resolved_family: String,
        /// Official source chunk identity.
        asset_identity: String,
    },
    /// Identifies an optional metadata component failure.
    #[non_exhaustive]
    Metadata {
        /// Metadata component being processed.
        component: MetadataComponent,
        /// Operation that failed.
        operation: MetadataOperation,
        /// Required optional-metadata recovery.
        recovery: Recovery,
    },
    /// Identifies one nested source occurrence and recovery.
    #[non_exhaustive]
    NestedSource {
        /// Asset occurrence chain from the scene to the nested source.
        asset_chain: Vec<AssetChainEntry>,
        /// Canonical nested paths that caused the event.
        paths: Vec<NestedPath>,
        /// Recovery applied to the nested source.
        recovery: Recovery,
    },
    /// A measured resource crossing with its exact nested source identity.
    #[non_exhaustive]
    NestedResourceLimit {
        /// Canonical limit field and configured ceiling.
        field: &'static str,
        /// Configured semantic ceiling.
        configured: ResourceValue,
        /// Unit measured by the ceiling.
        value_kind: ValueKind,
        /// Attempted or bounded lower-bound measurement.
        measurement: ResourceMeasurement,
        /// Enclosing authoritative asset occurrences.
        asset_chain: Vec<AssetChainEntry>,
        /// Exact rejected nested resource slot.
        paths: Vec<NestedPath>,
        /// Placeholder or omission selected at that slot.
        recovery: Recovery,
        /// Manifested nested-resource recipe for a placeholder, otherwise null.
        recipe_id: Option<String>,
    },
    /// Identifies a repaired media claim on a nested image reference.
    #[non_exhaustive]
    NestedAssetClaim {
        /// Asset occurrence chain from the scene to the nested source.
        asset_chain: Vec<AssetChainEntry>,
        /// Canonical path to the nested reference whose claim was repaired.
        paths: Vec<NestedPath>,
        /// Original media type stated by the nested data URL.
        claimed_media_type: String,
        /// Media type validated from the nested payload bytes.
        validated_media_type: String,
        /// Required repair recovery; the nested content remains present.
        recovery: Recovery,
    },
    /// Describes an emitted or fatal PDF raster fallback closure.
    #[non_exhaustive]
    PdfRasterFallback {
        /// Rasterized for a warning; null for a fatal fallback failure.
        recovery: Option<Recovery>,
        /// Logical closure bounds, when safely established.
        logical_bounds: Option<LogicalBounds>,
        /// Final fallback pixel width, when safely established.
        pixel_width: Option<u64>,
        /// Final fallback pixel height, when safely established.
        pixel_height: Option<u64>,
        /// Canonical nested paths that caused the closure.
        causes: Vec<NestedPath>,
    },
}

impl DiagnosticDetails {
    /// The recovery the details carry: the fixed one of a compatibility,
    /// asset, missing-glyph, metadata or nested-source detail, the optional
    /// one of a resource limit or PDF raster fallback (null when fatal), and
    /// none for the detail kinds that describe no recovery.
    pub(crate) fn recovery(&self) -> Option<Recovery> {
        match self {
            Self::Compatibility { recovery, .. }
            | Self::Asset { recovery, .. }
            | Self::MissingGlyph { recovery, .. }
            | Self::Metadata { recovery, .. }
            | Self::NestedSource { recovery, .. }
            | Self::NestedResourceLimit { recovery, .. }
            | Self::NestedAssetClaim { recovery, .. } => Some(*recovery),
            Self::ResourceLimit { recovery, .. } | Self::PdfRasterFallback { recovery, .. } => {
                *recovery
            }
            Self::ParseLimit { .. }
            | Self::RequestValue { .. }
            | Self::MemoryUnavailable { .. }
            | Self::FontUnavailable { .. }
            | Self::SvgFontSubsetFailure { .. } => None,
        }
    }

    /// The measured value and configured ceiling of a parser or resource limit
    /// crossing, which a failure's human message appends so that it says what
    /// its details say.
    pub(crate) fn limit_crossing(&self) -> Option<LimitCrossing> {
        let (field, configured, value_kind, measurement) = match self {
            Self::ParseLimit {
                field,
                configured,
                value_kind,
                measurement,
            } => (
                *field,
                ResourceValue::Integer(*configured),
                *value_kind,
                match *measurement {
                    Measurement::Attempted { value } => Measurement::Attempted {
                        value: ResourceValue::Integer(value),
                    },
                    Measurement::AtLeast { value } => Measurement::AtLeast {
                        value: ResourceValue::Integer(value),
                    },
                },
            ),
            Self::ResourceLimit {
                field,
                configured,
                value_kind,
                measurement,
                ..
            }
            | Self::NestedResourceLimit {
                field,
                configured,
                value_kind,
                measurement,
                ..
            } => (*field, *configured, *value_kind, *measurement),
            _ => return None,
        };
        Some(LimitCrossing {
            field,
            configured,
            value_kind,
            measurement,
        })
    }

    /// The fallback recipe identity the details name, when the compatibility
    /// or asset recovery painted a placeholder.
    pub(crate) fn recipe_id(&self) -> Option<&str> {
        match self {
            Self::Compatibility { recipe_id, .. }
            | Self::Asset { recipe_id, .. }
            | Self::NestedResourceLimit { recipe_id, .. } => recipe_id.as_deref(),
            Self::ParseLimit { .. }
            | Self::RequestValue { .. }
            | Self::ResourceLimit { .. }
            | Self::MemoryUnavailable { .. }
            | Self::MissingGlyph { .. }
            | Self::FontUnavailable { .. }
            | Self::SvgFontSubsetFailure { .. }
            | Self::Metadata { .. }
            | Self::NestedSource { .. }
            | Self::NestedAssetClaim { .. }
            | Self::PdfRasterFallback { .. } => None,
        }
    }
}

/// Stable stage vocabulary for a direct-SVG font subset failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum SvgFontSubsetStage {
    /// WOFF2 source decoding failed.
    Decode,
    /// Subset planning failed.
    Plan,
    /// Subset sfnt construction failed.
    Sfnt,
    /// WOFF2 encoding failed.
    Encode,
    /// Generated font validation failed.
    Validate,
}

/// Stable recovery applied after diagnosed compatibility loss.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum Recovery {
    /// Invalid or absent input was deterministically repaired.
    Repaired,
    /// Unsupported input was ignored.
    Ignored,
    /// Content was replaced by a compatibility placeholder.
    Placeholder,
    /// Content was omitted.
    Omitted,
    /// Unsafe or incompatible content was removed.
    Removed,
    /// Content was substituted deterministically.
    Substituted,
    /// Animation was flattened to a static frame.
    Flattened,
    /// A permitted isolated closure was rasterized.
    Rasterized,
    /// Optional metadata was omitted.
    MetadataOmitted,
}

impl Recovery {
    /// Returns the stable serialized spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Repaired => "repaired",
            Self::Ignored => "ignored",
            Self::Placeholder => "placeholder",
            Self::Omitted => "omitted",
            Self::Removed => "removed",
            Self::Substituted => "substituted",
            Self::Flattened => "flattened",
            Self::Rasterized => "rasterized",
            Self::MetadataOmitted => "metadata_omitted",
        }
    }

    /// The recovery the scene-surface manifest spells `recovery`, if any.
    pub(crate) fn from_manifest(recovery: &str) -> Option<Self> {
        match recovery {
            "repaired" => Some(Self::Repaired),
            "ignored" => Some(Self::Ignored),
            "placeholder" => Some(Self::Placeholder),
            "omitted" => Some(Self::Omitted),
            "removed" => Some(Self::Removed),
            "substituted" => Some(Self::Substituted),
            "flattened" => Some(Self::Flattened),
            "rasterized" => Some(Self::Rasterized),
            "metadata_omitted" => Some(Self::MetadataOmitted),
            _ => None,
        }
    }
}

impl fmt::Display for Recovery {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// One limit crossing in human form: `needs 400 pixels, but
/// max_raster_surface_pixels is 399`.
pub(crate) struct LimitCrossing {
    field: &'static str,
    configured: ResourceValue,
    value_kind: ValueKind,
    measurement: ResourceMeasurement,
}

impl fmt::Display for LimitCrossing {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (bound, value) = match self.measurement {
            Measurement::Attempted { value } => ("", value),
            Measurement::AtLeast { value } => ("at least ", value),
        };
        let unit = match self.value_kind {
            ValueKind::Bytes => " bytes",
            ValueKind::Pixels => " pixels",
            ValueKind::WorkUnits => " work units",
            ValueKind::Depth => " levels",
            ValueKind::Count | ValueKind::Dimension | ValueKind::Coordinate => "",
        };
        write!(
            formatter,
            "needs {bound}{}{unit}, but {} is {}",
            HumanValue(value),
            self.field,
            HumanValue(self.configured)
        )
    }
}

struct HumanValue(ResourceValue);

impl fmt::Display for HumanValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            ResourceValue::Integer(value) => value.fmt(formatter),
            ResourceValue::Finite(value) => value.get().fmt(formatter),
        }
    }
}

/// Unit used by a stable limit measurement.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum ValueKind {
    /// A byte count.
    Bytes,
    /// A structural-entry count.
    Count,
    /// A nesting depth.
    Depth,
    /// A pixel count.
    Pixels,
    /// A deterministic work-unit count.
    WorkUnits,
    /// A dimension.
    Dimension,
    /// A coordinate magnitude.
    Coordinate,
}

/// Stable attempted or lower-bound measurement.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[non_exhaustive]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Measurement<T = u64> {
    /// The exact attempted value is known.
    Attempted {
        /// The exact attempted value.
        value: T,
    },
    /// Bounded processing stopped after establishing this lower bound.
    AtLeast {
        /// The established lower bound.
        value: T,
    },
}

/// Finite schema-1 number with canonical JSON serialization.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FiniteValue(f64);

impl FiniteValue {
    /// Creates a value only when the supplied number is finite.
    pub(crate) fn new(value: f64) -> Option<Self> {
        value.is_finite().then_some(Self(value))
    }

    /// Returns the finite binary64 value.
    pub const fn get(self) -> f64 {
        self.0
    }
}

impl Serialize for FiniteValue {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let value = self.0;
        if value == 0.0 {
            serializer.serialize_u8(0)
        } else if (0.0..=9_007_199_254_740_992.0).contains(&value) && (value as u64) as f64 == value
        {
            serializer.serialize_u64(value as u64)
        } else if (-9_007_199_254_740_992.0..0.0).contains(&value) && (value as i64) as f64 == value
        {
            serializer.serialize_i64(value as i64)
        } else {
            serializer.serialize_f64(value)
        }
    }
}

/// Nonnegative integer or finite number used by semantic resource details.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum ResourceValue {
    /// An integer byte, count, depth, pixel, or work value.
    Integer(u64),
    /// A finite dimension or coordinate value.
    Finite(FiniteValue),
}

impl Serialize for ResourceValue {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match *self {
            Self::Integer(value) => serializer.serialize_u64(value),
            Self::Finite(value) => value.serialize(serializer),
        }
    }
}

/// Stable measurement form for integer, dimension, or coordinate resources.
pub type ResourceMeasurement = Measurement<ResourceValue>;

/// One entry in a nested asset occurrence chain.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AssetChainEntry {
    pub(crate) asset_id: Option<String>,
    pub(crate) occurrence: u64,
}

impl AssetChainEntry {
    /// Returns the scene file ID, or null for an anonymous nested occurrence.
    pub fn asset_id(&self) -> Option<&str> {
        self.asset_id.as_deref()
    }

    /// Returns the zero-based occurrence ordinal.
    pub const fn occurrence(&self) -> u64 {
        self.occurrence
    }
}

/// Kind of one canonical nested-source path segment.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum NestedSegmentKind {
    /// Nested document node.
    Node,
    /// Nested node attribute.
    Attribute,
    /// CSS rule.
    CssRule,
    /// CSS declaration.
    CssDeclaration,
    /// Nested resource slot.
    ResourceSlot,
    /// Effect input.
    EffectInput,
}

/// One segment of a canonical nested-source path.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct NestedPathSegment {
    pub(crate) kind: NestedSegmentKind,
    pub(crate) ordinal: u64,
    pub(crate) name: Option<String>,
}

impl NestedPathSegment {
    /// Returns the nested segment kind.
    pub const fn kind(&self) -> NestedSegmentKind {
        self.kind
    }

    /// Returns the zero-based segment ordinal.
    pub const fn ordinal(&self) -> u64 {
        self.ordinal
    }

    /// Returns the optional source name.
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }
}

/// Canonical identity path within one nested source.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct NestedPath {
    pub(crate) segments: Vec<NestedPathSegment>,
}

impl NestedPath {
    /// Returns the canonical nested path segments.
    pub fn segments(&self) -> &[NestedPathSegment] {
        &self.segments
    }
}

/// Finite logical bounds of a PDF raster fallback closure.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LogicalBounds {
    pub(crate) x: FiniteValue,
    pub(crate) y: FiniteValue,
    pub(crate) width: FiniteValue,
    pub(crate) height: FiniteValue,
}

impl Serialize for LogicalBounds {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut bounds = serializer.serialize_struct("LogicalBounds", 4)?;
        bounds.serialize_field("x", &self.x)?;
        bounds.serialize_field("y", &self.y)?;
        bounds.serialize_field("width", &self.width)?;
        bounds.serialize_field("height", &self.height)?;
        bounds.end()
    }
}

impl LogicalBounds {
    /// Returns the logical x coordinate.
    pub const fn x(&self) -> f64 {
        self.x.get()
    }

    /// Returns the logical y coordinate.
    pub const fn y(&self) -> f64 {
        self.y.get()
    }

    /// Returns the finite logical width.
    pub const fn width(&self) -> f64 {
        self.width.get()
    }

    /// Returns the finite logical height.
    pub const fn height(&self) -> f64 {
        self.height.get()
    }
}

/// Optional artifact metadata component.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum MetadataComponent {
    /// Renderer provenance.
    Provenance,
    /// Pre-metadata compatibility diagnostic summary.
    DiagnosticSummary,
    /// Canonicalized original scene document.
    EmbeddedScene,
    /// Format-neutral artifact manifest.
    Manifest,
}

/// Operation performed on an optional metadata component.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum MetadataOperation {
    /// Canonical serialization.
    Serialization,
    /// Deterministic compression.
    Compression,
    /// Format-specific embedding.
    Embedding,
}

/// Stable serializable view of a typed renderer failure.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ErrorDescriptor {
    family: ErrorFamily,
    category: Option<RenderErrorCategory>,
    code: ErrorCode,
    message: String,
    format: Option<OutputFormat>,
    element_id: Option<String>,
    json_pointer: Option<String>,
    details: Option<DiagnosticDetails>,
}

impl ErrorDescriptor {
    pub(crate) fn scene_parse(
        code: ErrorCode,
        message: String,
        json_pointer: Option<String>,
        details: Option<DiagnosticDetails>,
    ) -> Self {
        Self {
            family: ErrorFamily::SceneParse,
            category: None,
            code,
            message,
            format: None,
            element_id: None,
            json_pointer,
            details,
        }
    }

    pub(crate) fn request(code: ErrorCode, message: String, details: DiagnosticDetails) -> Self {
        Self {
            family: ErrorFamily::Request,
            category: None,
            code,
            message,
            format: None,
            element_id: None,
            json_pointer: None,
            details: Some(details),
        }
    }

    pub(crate) fn render(
        category: RenderErrorCategory,
        code: ErrorCode,
        message: String,
        format: OutputFormat,
        element_id: Option<String>,
        json_pointer: String,
        details: Option<DiagnosticDetails>,
    ) -> Self {
        Self {
            family: ErrorFamily::Render,
            category: Some(category),
            code,
            message,
            format: Some(format),
            element_id,
            json_pointer: Some(json_pointer),
            details,
        }
    }

    pub(crate) fn frame_batch(code: ErrorCode, message: String) -> Self {
        Self {
            family: ErrorFamily::FrameBatch,
            category: None,
            code,
            message,
            format: None,
            element_id: None,
            json_pointer: None,
            details: None,
        }
    }

    /// Returns the typed failure family.
    pub const fn family(&self) -> ErrorFamily {
        self.family
    }

    /// Returns the render-failure category, when applicable.
    pub const fn category(&self) -> Option<RenderErrorCategory> {
        self.category
    }

    /// Returns the stable machine-readable error code.
    pub const fn code(&self) -> ErrorCode {
        self.code
    }

    /// Returns the human-readable error message.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Returns the selected output format, when applicable.
    pub const fn format(&self) -> Option<OutputFormat> {
        self.format
    }

    /// Returns the normalized element ID, when applicable.
    pub fn element_id(&self) -> Option<&str> {
        self.element_id.as_deref()
    }

    /// Returns the scene-document JSON pointer, when applicable.
    pub fn json_pointer(&self) -> Option<&str> {
        self.json_pointer.as_deref()
    }

    /// Returns typed details required by the stable error code.
    pub const fn details(&self) -> Option<&DiagnosticDetails> {
        self.details.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_detail_kind_serializes_in_its_declared_member_order() {
        let finite = |value| FiniteValue::new(value).expect("test value is finite");
        let path = NestedPath {
            segments: vec![NestedPathSegment {
                kind: NestedSegmentKind::Node,
                ordinal: 1,
                name: None,
            }],
        };
        let cases = [
            (
                DiagnosticDetails::ParseLimit {
                    field: "max_scene_bytes",
                    configured: 67_108_864,
                    value_kind: ValueKind::Bytes,
                    measurement: Measurement::AtLeast { value: 67_108_865 },
                },
                concat!(
                    "{\"kind\":\"parse_limit\",\"field\":\"max_scene_bytes\",",
                    "\"configured\":67108864,\"value_kind\":\"bytes\",",
                    "\"measurement\":{\"kind\":\"at_least\",\"value\":67108865}}"
                ),
            ),
            (
                DiagnosticDetails::RequestValue {
                    field: "supplemental_assets",
                    asset_id: Some("image-1".to_owned()),
                    option: Some("media_type"),
                },
                concat!(
                    "{\"kind\":\"request_value\",\"field\":\"supplemental_assets\",",
                    "\"asset_id\":\"image-1\",\"option\":\"media_type\"}"
                ),
            ),
            (
                DiagnosticDetails::Compatibility {
                    recovery: Recovery::Substituted,
                    recipe_id: Some("official.embeddable-offline@sha256:d1d9".to_owned()),
                },
                concat!(
                    "{\"kind\":\"compatibility\",\"recovery\":\"substituted\",",
                    "\"recipe_id\":\"official.embeddable-offline@sha256:d1d9\"}"
                ),
            ),
            (
                DiagnosticDetails::Asset {
                    asset_id: "image-1".to_owned(),
                    claimed_media_type: Some("image/jpeg".to_owned()),
                    validated_media_type: Some("image/png".to_owned()),
                    recovery: Recovery::Repaired,
                    recipe_id: None,
                },
                concat!(
                    "{\"kind\":\"asset\",\"asset_id\":\"image-1\",",
                    "\"claimed_media_type\":\"image/jpeg\",",
                    "\"validated_media_type\":\"image/png\",",
                    "\"recovery\":\"repaired\",\"recipe_id\":null}"
                ),
            ),
            (
                DiagnosticDetails::ResourceLimit {
                    field: "max_coordinate_magnitude",
                    configured: ResourceValue::Finite(finite(1e9)),
                    value_kind: ValueKind::Coordinate,
                    measurement: ResourceMeasurement::Attempted {
                        value: ResourceValue::Finite(finite(1_000_000_000.5)),
                    },
                    recovery: Some(Recovery::Omitted),
                },
                concat!(
                    "{\"kind\":\"resource_limit\",",
                    "\"field\":\"max_coordinate_magnitude\",",
                    "\"configured\":1000000000,\"value_kind\":\"coordinate\",",
                    "\"measurement\":{\"kind\":\"attempted\",",
                    "\"value\":1000000000.5},\"recovery\":\"omitted\"}"
                ),
            ),
            (
                DiagnosticDetails::MemoryUnavailable {
                    subject_kind: "output_buffer".to_owned(),
                    subject: "svg".to_owned(),
                },
                concat!(
                    "{\"kind\":\"memory_unavailable\",",
                    "\"subject_kind\":\"output_buffer\",\"subject\":\"svg\"}"
                ),
            ),
            (
                DiagnosticDetails::MissingGlyph {
                    codepoint: 0x10ffff,
                    recovery: Recovery::Substituted,
                },
                concat!(
                    "{\"kind\":\"missing_glyph\",\"codepoint\":1114111,",
                    "\"recovery\":\"substituted\"}"
                ),
            ),
            (
                DiagnosticDetails::SvgFontSubsetFailure {
                    stage: SvgFontSubsetStage::Validate,
                    resolved_family: "Excalifont".to_owned(),
                    asset_identity: "fonts/Excalifont/chunk.woff2@sha256:abc".to_owned(),
                },
                concat!(
                    "{\"kind\":\"svg_font_subset_failure\",\"stage\":\"validate\",",
                    "\"resolved_family\":\"Excalifont\",",
                    "\"asset_identity\":\"fonts/Excalifont/chunk.woff2@sha256:abc\"}"
                ),
            ),
            (
                DiagnosticDetails::FontUnavailable {
                    resolved_family: "Xiaolai".to_owned(),
                    asset_identity: "fonts/Xiaolai/chunk.woff2@sha256:abc".to_owned(),
                },
                concat!(
                    "{\"kind\":\"font_unavailable\",\"resolved_family\":\"Xiaolai\",",
                    "\"asset_identity\":\"fonts/Xiaolai/chunk.woff2@sha256:abc\"}"
                ),
            ),
            (
                DiagnosticDetails::Metadata {
                    component: MetadataComponent::EmbeddedScene,
                    operation: MetadataOperation::Compression,
                    recovery: Recovery::MetadataOmitted,
                },
                concat!(
                    "{\"kind\":\"metadata\",\"component\":\"embedded_scene\",",
                    "\"operation\":\"compression\",",
                    "\"recovery\":\"metadata_omitted\"}"
                ),
            ),
            (
                DiagnosticDetails::NestedSource {
                    asset_chain: vec![AssetChainEntry {
                        asset_id: Some("image-1".to_owned()),
                        occurrence: 0,
                    }],
                    paths: vec![path.clone()],
                    recovery: Recovery::Removed,
                },
                concat!(
                    "{\"kind\":\"nested_source\",",
                    "\"asset_chain\":[{\"asset_id\":\"image-1\",\"occurrence\":0}],",
                    "\"paths\":[{\"segments\":[{\"kind\":\"node\",",
                    "\"ordinal\":1,\"name\":null}]}],\"recovery\":\"removed\"}"
                ),
            ),
            (
                DiagnosticDetails::NestedResourceLimit {
                    field: "max_nested_asset_depth",
                    configured: ResourceValue::Integer(8),
                    value_kind: ValueKind::Depth,
                    measurement: ResourceMeasurement::AtLeast {
                        value: ResourceValue::Integer(9),
                    },
                    asset_chain: vec![AssetChainEntry {
                        asset_id: None,
                        occurrence: 2,
                    }],
                    paths: vec![path.clone()],
                    recovery: Recovery::Omitted,
                    recipe_id: None,
                },
                concat!(
                    "{\"kind\":\"nested_resource_limit\",",
                    "\"field\":\"max_nested_asset_depth\",\"configured\":8,",
                    "\"value_kind\":\"depth\",",
                    "\"measurement\":{\"kind\":\"at_least\",\"value\":9},",
                    "\"asset_chain\":[{\"asset_id\":null,\"occurrence\":2}],",
                    "\"paths\":[{\"segments\":[{\"kind\":\"node\",",
                    "\"ordinal\":1,\"name\":null}]}],\"recovery\":\"omitted\",",
                    "\"recipe_id\":null}"
                ),
            ),
            (
                DiagnosticDetails::NestedAssetClaim {
                    asset_chain: vec![AssetChainEntry {
                        asset_id: Some("image-1".to_owned()),
                        occurrence: 0,
                    }],
                    paths: vec![path.clone()],
                    claimed_media_type: "image/jpeg".to_owned(),
                    validated_media_type: "image/png".to_owned(),
                    recovery: Recovery::Repaired,
                },
                concat!(
                    "{\"kind\":\"nested_asset_claim\",",
                    "\"asset_chain\":[{\"asset_id\":\"image-1\",\"occurrence\":0}],",
                    "\"paths\":[{\"segments\":[{\"kind\":\"node\",",
                    "\"ordinal\":1,\"name\":null}]}],",
                    "\"claimed_media_type\":\"image/jpeg\",",
                    "\"validated_media_type\":\"image/png\",\"recovery\":\"repaired\"}"
                ),
            ),
            (
                DiagnosticDetails::PdfRasterFallback {
                    recovery: None,
                    logical_bounds: Some(LogicalBounds {
                        x: finite(-0.0),
                        y: finite(1.5),
                        width: finite(10.0),
                        height: finite(20.0),
                    }),
                    pixel_width: None,
                    pixel_height: None,
                    causes: vec![path],
                },
                concat!(
                    "{\"kind\":\"pdf_raster_fallback\",\"recovery\":null,",
                    "\"logical_bounds\":{\"x\":0,\"y\":1.5,",
                    "\"width\":10,\"height\":20},\"pixel_width\":null,",
                    "\"pixel_height\":null,\"causes\":[{\"segments\":[{",
                    "\"kind\":\"node\",\"ordinal\":1,\"name\":null}]}]}"
                ),
            ),
        ];

        let mut kinds = Vec::new();
        for (details, expected) in cases {
            // Naming every variant here fails the build when one is added
            // without a case above.
            let kind = match &details {
                DiagnosticDetails::ParseLimit { .. } => "parse_limit",
                DiagnosticDetails::RequestValue { .. } => "request_value",
                DiagnosticDetails::Compatibility { .. } => "compatibility",
                DiagnosticDetails::Asset { .. } => "asset",
                DiagnosticDetails::ResourceLimit { .. } => "resource_limit",
                DiagnosticDetails::MemoryUnavailable { .. } => "memory_unavailable",
                DiagnosticDetails::MissingGlyph { .. } => "missing_glyph",
                DiagnosticDetails::FontUnavailable { .. } => "font_unavailable",
                DiagnosticDetails::SvgFontSubsetFailure { .. } => "svg_font_subset_failure",
                DiagnosticDetails::Metadata { .. } => "metadata",
                DiagnosticDetails::NestedSource { .. } => "nested_source",
                DiagnosticDetails::NestedResourceLimit { .. } => "nested_resource_limit",
                DiagnosticDetails::NestedAssetClaim { .. } => "nested_asset_claim",
                DiagnosticDetails::PdfRasterFallback { .. } => "pdf_raster_fallback",
            };
            kinds.push(kind);
            assert_eq!(
                serde_json::to_string(&details).expect("serialize schema-1 details"),
                expected
            );
        }
        kinds.sort_unstable();
        kinds.dedup();
        assert_eq!(kinds.len(), 14, "one case for every detail kind");
    }

    #[test]
    fn stable_numeric_values_reject_non_finite_input() {
        assert_eq!(FiniteValue::new(f64::NAN), None);
        assert_eq!(FiniteValue::new(f64::INFINITY), None);
        assert_eq!(FiniteValue::new(f64::NEG_INFINITY), None);
        assert_eq!(FiniteValue::new(-0.0).map(FiniteValue::get), Some(-0.0));
    }
}
