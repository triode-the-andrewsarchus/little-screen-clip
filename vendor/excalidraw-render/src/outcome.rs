use std::collections::TryReserveError;
use std::error::Error;
use std::fmt;

use serde::Serialize;

use crate::diagnostic_registry::DiagnosticKind;
use crate::{
    DiagnosticDetails, ErrorCode, ErrorDescriptor, OutputFormat, ReferenceProfileIdentity,
};

/// String-backed stable compatibility diagnostic code.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct DiagnosticCode(&'static str);

impl DiagnosticCode {
    /// A missing source element ID was repaired.
    pub const NORMALIZATION_MISSING_ELEMENT_ID: Self = Self("normalization.missing-element-id");
    /// A later duplicate source element ID was repaired.
    pub const NORMALIZATION_DUPLICATE_ELEMENT_ID: Self = Self("normalization.duplicate-element-id");
    /// A missing required timestamp was repaired to canonical zero.
    pub const NORMALIZATION_MISSING_TIMESTAMP: Self = Self("normalization.missing-timestamp");
    /// A zero Rough seed was repaired deterministically.
    pub const NORMALIZATION_ZERO_ROUGH_SEED: Self = Self("normalization.zero-rough-seed");
    /// A cycle-closing relationship edge was removed.
    pub const NORMALIZATION_RELATIONSHIP_CYCLE: Self = Self("normalization.relationship-cycle");
    /// An unknown element type used the compatibility fallback.
    pub const COMPATIBILITY_UNSUPPORTED_ELEMENT_TYPE: Self =
        Self("compatibility.unsupported-element-type");
    /// A malformed element was replaced or omitted.
    pub const COMPATIBILITY_MALFORMED_ELEMENT: Self = Self("compatibility.malformed-element");
    /// An unsupported property was ignored.
    pub const COMPATIBILITY_UNSUPPORTED_PROPERTY: Self = Self("compatibility.unsupported-property");
    /// An unsupported property value used its declared recovery.
    pub const COMPATIBILITY_UNSUPPORTED_PROPERTY_VALUE: Self =
        Self("compatibility.unsupported-property-value");
    /// A known but unavailable element was replaced or omitted.
    pub const COMPATIBILITY_KNOWN_ELEMENT_REPLACED: Self =
        Self("compatibility.known-element-replaced");
    /// A scene-embedded asset remained authoritative over a supplement.
    pub const ASSET_SUPPLEMENT_CONFLICT: Self = Self("asset.supplement-conflict");
    /// Sniffed payload type replaced a mismatched advisory media type.
    pub const ASSET_MEDIA_TYPE_MISMATCH: Self = Self("asset.media-type-mismatch");
    /// A referenced scene file record was malformed.
    pub const ASSET_MALFORMED_FILE_RECORD: Self = Self("asset.malformed-file-record");
    /// A referring image had no authoritative file payload.
    pub const IMAGE_MISSING_FILE: Self = Self("image.missing-file");
    /// A forbidden image resource was removed or replaced.
    pub const IMAGE_FORBIDDEN_RESOURCE: Self = Self("image.forbidden-resource");
    /// A rendered animated image occurrence was flattened.
    pub const IMAGE_ANIMATION_FLATTENED: Self = Self("image.animation-flattened");
    /// An unsupported image format was omitted or replaced.
    pub const IMAGE_UNSUPPORTED_FORMAT: Self = Self("image.unsupported-format");
    /// Image decoding failed and used format-specific recovery.
    pub const IMAGE_DECODE_FAILED: Self = Self("image.decode-failed");
    /// Unsafe nested SVG content was removed.
    pub const IMAGE_UNSAFE_SVG_CONTENT_REMOVED: Self = Self("image.unsafe-svg-content-removed");
    /// An unsafe element link was omitted.
    pub const LINK_UNSAFE_OMITTED: Self = Self("link.unsafe-omitted");
    /// An invalid color was substituted deterministically.
    pub const COLOR_INVALID: Self = Self("color.invalid");
    /// A local font request used bundled Liberation Sans.
    pub const FONT_LIBERATION_SANS_SUBSTITUTED: Self = Self("font.liberation-sans-substituted");
    /// Emoji text used bundled Noto Emoji.
    pub const FONT_NOTO_EMOJI_SUBSTITUTED: Self = Self("font.noto-emoji-substituted");
    /// A missing glyph was replaced deterministically.
    pub const FONT_MISSING_GLYPH: Self = Self("font.missing-glyph");
    /// An over-complex element was replaced or omitted.
    pub const RESOURCE_ELEMENT_COMPLEXITY_REPLACED: Self =
        Self("resource.element-complexity-replaced");
    /// Element-local semantic work used local recovery.
    pub const RESOURCE_ELEMENT_WORK_EXCEEDED: Self = Self("resource.element-work-exceeded");
    /// One asset occurrence exceeded its byte ceiling.
    pub const RESOURCE_ASSET_BYTES_EXCEEDED: Self = Self("resource.asset-bytes-exceeded");
    /// One semantic asset exceeded its work ceiling.
    pub const RESOURCE_ASSET_WORK_EXCEEDED: Self = Self("resource.asset-work-exceeded");
    /// Raster dimensions exceeded the local image ceiling.
    pub const RESOURCE_IMAGE_DIMENSIONS_EXCEEDED: Self = Self("resource.image-dimensions-exceeded");
    /// Decoded image pixels exceeded the local asset ceiling.
    pub const RESOURCE_IMAGE_PIXELS_EXCEEDED: Self = Self("resource.image-pixels-exceeded");
    /// Nested SVG complexity exceeded its local node ceiling.
    pub const RESOURCE_NESTED_SVG_COMPLEXITY_EXCEEDED: Self =
        Self("resource.nested-svg-complexity-exceeded");
    /// Nested asset discovery exceeded its local depth ceiling.
    pub const RESOURCE_NESTED_ASSET_DEPTH_EXCEEDED: Self =
        Self("resource.nested-asset-depth-exceeded");
    /// A coordinate exceeded its local magnitude ceiling.
    pub const RESOURCE_COORDINATE_RANGE_EXCEEDED: Self = Self("resource.coordinate-range-exceeded");
    /// Optional metadata was omitted after a resource crossing.
    pub const RESOURCE_OPTIONAL_METADATA_OMITTED: Self = Self("resource.optional-metadata-omitted");
    /// Provenance serialization failed and was omitted.
    pub const METADATA_PROVENANCE_SERIALIZATION_FAILED: Self =
        Self("metadata.provenance-serialization-failed");
    /// Diagnostic-summary serialization failed and was omitted.
    pub const METADATA_DIAGNOSTIC_SUMMARY_SERIALIZATION_FAILED: Self =
        Self("metadata.diagnostic-summary-serialization-failed");
    /// Embedded-scene serialization failed and was omitted.
    pub const METADATA_SCENE_SERIALIZATION_FAILED: Self =
        Self("metadata.scene-serialization-failed");
    /// Embedded-scene compression failed and was omitted.
    pub const METADATA_SCENE_COMPRESSION_FAILED: Self = Self("metadata.scene-compression-failed");
    /// Embedded-scene carrier insertion failed and was omitted.
    pub const METADATA_SCENE_EMBEDDING_FAILED: Self = Self("metadata.scene-embedding-failed");
    /// Artifact-manifest serialization failed.
    pub const METADATA_MANIFEST_SERIALIZATION_FAILED: Self =
        Self("metadata.manifest-serialization-failed");
    /// Artifact-manifest carrier insertion failed.
    pub const METADATA_MANIFEST_EMBEDDING_FAILED: Self = Self("metadata.manifest-embedding-failed");
    /// A permitted isolated PDF closure was rasterized.
    pub const PDF_RASTER_FALLBACK: Self = Self("pdf.raster-fallback");

    /// The complete revision-1 registry (spec section 11.3), in the
    /// manifest's order. The registry tests check it against the manifest's
    /// registry mappings.
    pub(crate) const ALL: [Self; 42] = [
        Self::NORMALIZATION_MISSING_ELEMENT_ID,
        Self::NORMALIZATION_DUPLICATE_ELEMENT_ID,
        Self::NORMALIZATION_MISSING_TIMESTAMP,
        Self::NORMALIZATION_ZERO_ROUGH_SEED,
        Self::NORMALIZATION_RELATIONSHIP_CYCLE,
        Self::COMPATIBILITY_UNSUPPORTED_ELEMENT_TYPE,
        Self::COMPATIBILITY_MALFORMED_ELEMENT,
        Self::COMPATIBILITY_UNSUPPORTED_PROPERTY,
        Self::COMPATIBILITY_UNSUPPORTED_PROPERTY_VALUE,
        Self::COMPATIBILITY_KNOWN_ELEMENT_REPLACED,
        Self::ASSET_SUPPLEMENT_CONFLICT,
        Self::ASSET_MEDIA_TYPE_MISMATCH,
        Self::ASSET_MALFORMED_FILE_RECORD,
        Self::IMAGE_MISSING_FILE,
        Self::IMAGE_FORBIDDEN_RESOURCE,
        Self::IMAGE_ANIMATION_FLATTENED,
        Self::IMAGE_UNSUPPORTED_FORMAT,
        Self::IMAGE_DECODE_FAILED,
        Self::IMAGE_UNSAFE_SVG_CONTENT_REMOVED,
        Self::LINK_UNSAFE_OMITTED,
        Self::COLOR_INVALID,
        Self::FONT_LIBERATION_SANS_SUBSTITUTED,
        Self::FONT_NOTO_EMOJI_SUBSTITUTED,
        Self::FONT_MISSING_GLYPH,
        Self::RESOURCE_ELEMENT_COMPLEXITY_REPLACED,
        Self::RESOURCE_ELEMENT_WORK_EXCEEDED,
        Self::RESOURCE_ASSET_BYTES_EXCEEDED,
        Self::RESOURCE_ASSET_WORK_EXCEEDED,
        Self::RESOURCE_IMAGE_DIMENSIONS_EXCEEDED,
        Self::RESOURCE_IMAGE_PIXELS_EXCEEDED,
        Self::RESOURCE_NESTED_SVG_COMPLEXITY_EXCEEDED,
        Self::RESOURCE_NESTED_ASSET_DEPTH_EXCEEDED,
        Self::RESOURCE_COORDINATE_RANGE_EXCEEDED,
        Self::RESOURCE_OPTIONAL_METADATA_OMITTED,
        Self::METADATA_PROVENANCE_SERIALIZATION_FAILED,
        Self::METADATA_DIAGNOSTIC_SUMMARY_SERIALIZATION_FAILED,
        Self::METADATA_SCENE_SERIALIZATION_FAILED,
        Self::METADATA_SCENE_COMPRESSION_FAILED,
        Self::METADATA_SCENE_EMBEDDING_FAILED,
        Self::METADATA_MANIFEST_SERIALIZATION_FAILED,
        Self::METADATA_MANIFEST_EMBEDDING_FAILED,
        Self::PDF_RASTER_FALLBACK,
    ];

    /// The registered code with the stable spelling `code`, or `None` when
    /// the registry has no such code. The sealed scene-surface policy names
    /// its codes as strings; this is how they become codes, so a spelling the
    /// registry does not know cannot be reported.
    pub(crate) fn from_stable_str(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|known| known.as_str() == code)
    }

    /// Returns the stable serialized spelling.
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

impl fmt::Display for DiagnosticCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Severity of one compatibility diagnostic occurrence.
///
/// Severities are ordered from least to most severe: `Info < Warning`.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticSeverity {
    /// Informational compatibility behavior.
    Info,
    /// Recoverable compatibility loss or repair.
    Warning,
}

impl DiagnosticSeverity {
    /// Returns the stable serialized spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warning => "warning",
        }
    }
}

impl fmt::Display for DiagnosticSeverity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Observable effect class of a compatibility diagnostic.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticEffect {
    /// Submitted input was repaired, selected, or ignored.
    InputRepair,
    /// Appearance may differ.
    Visual,
    /// Links, text, animation, or vector semantics may differ.
    Semantic,
    /// Carried metadata may differ.
    Metadata,
}

impl DiagnosticEffect {
    /// Returns the stable serialized spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InputRepair => "input_repair",
            Self::Visual => "visual",
            Self::Semantic => "semantic",
            Self::Metadata => "metadata",
        }
    }

    /// The effect the scene-surface manifest spells `effect`, if any.
    pub(crate) fn from_manifest(effect: &str) -> Option<Self> {
        match effect {
            "input_repair" => Some(Self::InputRepair),
            "visual" => Some(Self::Visual),
            "semantic" => Some(Self::Semantic),
            "metadata" => Some(Self::Metadata),
            _ => None,
        }
    }
}

impl fmt::Display for DiagnosticEffect {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// One compatibility diagnostic as the stage that diagnosed it emits it:
/// everything the report carries except the position the session assigns
/// when it retains it. Built only from a [`DiagnosticKind`], so the code,
/// severity and details agree by construction, and turned into a
/// [`CompatibilityDiagnostic`] only by [`PendingDiagnostic::retained`], so a
/// diagnostic in a report always has a real order.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PendingDiagnostic {
    code: DiagnosticCode,
    severity: DiagnosticSeverity,
    format: OutputFormat,
    effects: Vec<DiagnosticEffect>,
    element_id: Option<String>,
    json_pointer: String,
    message: String,
    details: Option<Box<DiagnosticDetails>>,
}

fn set_identity_effects(
    effects: &mut Vec<DiagnosticEffect>,
    format: OutputFormat,
    visual: bool,
) -> Result<(), TryReserveError> {
    effects.try_reserve(3)?;
    effects.clear();
    effects.push(DiagnosticEffect::InputRepair);
    if visual {
        effects.push(DiagnosticEffect::Visual);
        if format != OutputFormat::Png {
            effects.push(DiagnosticEffect::Semantic);
        }
    }
    Ok(())
}

impl PendingDiagnostic {
    pub(crate) fn omit_unbounded_known_element_fallback(&mut self) -> Result<(), TryReserveError> {
        if self.code != DiagnosticCode::COMPATIBILITY_UNSUPPORTED_PROPERTY_VALUE {
            return Ok(());
        }
        if let Some(details) = self.details.as_deref_mut() {
            if matches!(details, DiagnosticDetails::Compatibility { recovery:crate::Recovery::Placeholder, recipe_id:Some(recipe) }
                if recipe==crate::diagnostic_registry::FallbackRecipe::KnownElementUnavailable.id())
            {
                *details = DiagnosticKind::UnsupportedPropertyValue {
                    recovery: crate::Recovery::Omitted,
                    recipe: None,
                }
                .into_details()?;
            }
        }
        Ok(())
    }
    pub(crate) fn set_identity_effects(&mut self, visual: bool) -> Result<(), TryReserveError> {
        set_identity_effects(&mut self.effects, self.format, visual)
    }
    /// The diagnostic `kind` describes, produced for `format` with the
    /// `effects` the report will list, attributed to `element_id` at the RFC
    /// 6901 `json_pointer`, with the non-normative `message`.
    pub(crate) fn new(
        kind: DiagnosticKind<'_>,
        format: OutputFormat,
        effects: &[DiagnosticEffect],
        element_id: Option<&str>,
        json_pointer: &str,
        message: &str,
    ) -> Result<Self, TryReserveError> {
        let mut owned_effects = Vec::new();
        owned_effects.try_reserve_exact(effects.len())?;
        owned_effects.extend_from_slice(effects);
        Ok(Self {
            code: kind.code(),
            severity: kind.severity(),
            format,
            effects: owned_effects,
            element_id: element_id.map(try_owned).transpose()?,
            json_pointer: try_owned(json_pointer)?,
            message: try_owned(message)?,
            details: Some(Box::new(kind.into_details()?)),
        })
    }

    pub(crate) const fn code(&self) -> DiagnosticCode {
        self.code
    }

    pub(crate) fn element_id(&self) -> Option<&str> {
        self.element_id.as_deref()
    }

    pub(crate) fn json_pointer(&self) -> &str {
        &self.json_pointer
    }

    /// Re-select a file-level recovery after the frame's referring images are
    /// known. Preserve the original source pointer, message and ordering data.
    pub(crate) fn scope_malformed_image_recovery(
        &mut self,
        error_status: impl FnOnce(&str) -> bool,
    ) -> Result<(), TryReserveError> {
        if self.code != DiagnosticCode::ASSET_MALFORMED_FILE_RECORD {
            return Ok(());
        }
        if let Some(DiagnosticDetails::Asset { asset_id, .. }) = self.details.as_deref() {
            let kind = DiagnosticKind::ImageUnavailable {
                failure: crate::diagnostic_registry::ImageFailure::MalformedFileRecord,
                asset_id,
                recovery: crate::diagnostic_registry::ImageRecovery::for_format(
                    self.format,
                    error_status(asset_id),
                ),
            };
            self.details = Some(Box::new(kind.into_details()?));
        }
        Ok(())
    }

    pub(crate) fn scope_forbidden_resource_recovery(
        &mut self,
        has_viewport: impl FnOnce(&str) -> bool,
    ) -> Result<(), TryReserveError> {
        if self.code != DiagnosticCode::IMAGE_FORBIDDEN_RESOURCE {
            return Ok(());
        }
        if let Some(details) = self.details.as_deref_mut() {
            if let DiagnosticDetails::Asset { asset_id, .. } = details {
                let kind = DiagnosticKind::ForbiddenResource {
                    asset_id,
                    recovery: crate::diagnostic_registry::LocalRecovery::placeholder_if(
                        has_viewport(asset_id),
                    ),
                };
                *details = kind.into_details()?;
            }
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn effects(&self) -> &[DiagnosticEffect] {
        &self.effects
    }

    #[cfg(test)]
    pub(crate) fn details(&self) -> Option<&DiagnosticDetails> {
        self.details.as_deref()
    }

    /// The owned element ID and pointer, for a failure that names where the
    /// diagnostic could not be retained.
    pub(crate) fn into_location(self) -> (Option<String>, String) {
        (self.element_id, self.json_pointer)
    }

    /// The diagnostic as the session retains it at `order` in its report.
    pub(crate) fn retained(self, order: DiagnosticOrder) -> CompatibilityDiagnostic {
        CompatibilityDiagnostic {
            code: self.code,
            severity: self.severity,
            format: self.format,
            effects: self.effects,
            element_id: self.element_id,
            json_pointer: self.json_pointer,
            message: self.message,
            details: self.details,
            order,
        }
    }
}

fn try_owned(value: &str) -> Result<String, TryReserveError> {
    let mut owned = String::new();
    owned.try_reserve_exact(value.len())?;
    owned.push_str(value);
    Ok(owned)
}

/// Stable serializable view of one compatibility diagnostic occurrence.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CompatibilityDiagnostic {
    code: DiagnosticCode,
    severity: DiagnosticSeverity,
    format: OutputFormat,
    effects: Vec<DiagnosticEffect>,
    element_id: Option<String>,
    json_pointer: String,
    message: String,
    details: Option<Box<DiagnosticDetails>>,
    #[serde(skip)]
    order: DiagnosticOrder,
}

/// Where the session placed a retained diagnostic in the total report order
/// of spec section 11.3: the source owner and member position the emitting
/// stage attributed it to, the order it was retained in, and whether a frame
/// export attributes it to the scene or to the target. Only
/// [`DiagnosticOrder::retained`] builds one.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct DiagnosticOrder {
    pub(crate) owner_index: u64,
    pub(crate) source_position: u64,
    pub(crate) retention_ordinal: u64,
    pub(crate) attribution: DiagnosticAttribution,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum DiagnosticAttribution {
    SceneGlobal,
    Target,
}

impl DiagnosticOrder {
    /// The order the session gives a diagnostic it retains as the
    /// `retention_ordinal`th of its report: the source owner and member
    /// position the emitting stage attributed it to, or last among the owners
    /// or positions when the stage knew neither, so root-owned and unowned
    /// diagnostics follow every source-owned one.
    pub(crate) fn retained(
        owner_index: Option<u64>,
        source_position: Option<u64>,
        retention_ordinal: u64,
        attribution: DiagnosticAttribution,
    ) -> Self {
        Self {
            owner_index: owner_index.unwrap_or(u64::MAX),
            source_position: source_position.unwrap_or(u64::MAX),
            retention_ordinal,
            attribution,
        }
    }
}

impl CompatibilityDiagnostic {
    /// A rejected parent frame removes this placeholder and its surviving link.
    pub(crate) fn omit_surface_fallback(&mut self, has_link: bool) -> Result<(), TryReserveError> {
        if let Some(DiagnosticDetails::Compatibility {
            recovery,
            recipe_id,
        }) = self.details.as_deref_mut()
        {
            if *recovery == crate::Recovery::Placeholder {
                if self.code == DiagnosticCode::COMPATIBILITY_MALFORMED_ELEMENT
                    && has_link
                    && self.format != OutputFormat::Png
                    && !self.effects.contains(&DiagnosticEffect::Semantic)
                {
                    self.effects.try_reserve(1)?;
                    self.effects.push(DiagnosticEffect::Semantic);
                }
                *recovery = crate::Recovery::Omitted;
                *recipe_id = None;
            }
        }
        Ok(())
    }
    pub(crate) fn set_forbidden_resource_recovery(
        &mut self,
        paints: bool,
    ) -> Result<(), TryReserveError> {
        if self.code == DiagnosticCode::IMAGE_FORBIDDEN_RESOURCE {
            if let Some(details) = self.details.as_deref_mut() {
                if let DiagnosticDetails::Asset { asset_id, .. } = details {
                    *details = DiagnosticKind::ForbiddenResource {
                        asset_id,
                        recovery: crate::diagnostic_registry::LocalRecovery::placeholder_if(paints),
                    }
                    .into_details()?;
                }
            }
        }
        Ok(())
    }
    pub(crate) fn set_identity_effects(&mut self, visual: bool) -> Result<(), TryReserveError> {
        set_identity_effects(&mut self.effects, self.format, visual)
    }
    /// Returns the stable machine-readable diagnostic code.
    pub const fn code(&self) -> DiagnosticCode {
        self.code
    }

    /// Returns the diagnostic severity.
    pub const fn severity(&self) -> DiagnosticSeverity {
        self.severity
    }

    /// Returns the format whose projection produced this diagnostic.
    pub const fn format(&self) -> OutputFormat {
        self.format
    }

    /// Returns the nonempty canonical effect list.
    pub fn effects(&self) -> &[DiagnosticEffect] {
        &self.effects
    }

    /// Returns the normalized source element ID, when applicable.
    pub fn element_id(&self) -> Option<&str> {
        self.element_id.as_deref()
    }

    /// Returns the required RFC 6901 scene pointer.
    pub fn json_pointer(&self) -> &str {
        &self.json_pointer
    }

    /// Returns the human-readable explanation.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Returns typed details required by the diagnostic code.
    pub fn details(&self) -> Option<&DiagnosticDetails> {
        self.details.as_deref()
    }

    /// The details, for the one recovery the session revises after
    /// retention: a resource placeholder it later cannot paint becomes an
    /// omission.
    pub(crate) fn details_mut(&mut self) -> Option<&mut DiagnosticDetails> {
        self.details.as_deref_mut()
    }

    /// The position the session retained this diagnostic at.
    pub(crate) const fn order(&self) -> DiagnosticOrder {
        self.order
    }

    /// Attributes the diagnostic to the frame export target instead of the
    /// scene, once frame scoping knows the owner participates.
    pub(crate) fn attribute_to_target(&mut self) {
        self.order.attribution = DiagnosticAttribution::Target;
    }

    /// Whether the diagnosed occurrence has visual placeholder/omission
    /// recovery. A field occurrence need not replace its entire source element.
    pub(crate) fn replaces_occurrence(&self) -> bool {
        self.effects.contains(&DiagnosticEffect::Visual)
            && matches!(
                self.details().and_then(DiagnosticDetails::recovery),
                Some(crate::Recovery::Placeholder | crate::Recovery::Omitted)
            )
    }
}

impl fmt::Display for CompatibilityDiagnostic {
    /// One line for a person: `warning: MESSAGE [CODE] (element ID)`, the
    /// element part only when the diagnostic names one.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}: {} [{}]",
            self.severity, self.message, self.code
        )?;
        if let Some(element_id) = &self.element_id {
            write!(formatter, " (element {element_id})")?;
        }
        Ok(())
    }
}

/// Complete ordered compatibility diagnostics for one render transaction.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CompatibilityReport {
    pub(crate) schema_version: u32,
    pub(crate) diagnostics: Vec<CompatibilityDiagnostic>,
}

impl CompatibilityReport {
    /// Creates a standalone empty schema-1 report.
    pub const fn empty() -> Self {
        Self {
            schema_version: 1,
            diagnostics: Vec::new(),
        }
    }

    /// Returns the stable report schema version.
    pub const fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// Returns the complete diagnostics in canonical order.
    pub fn diagnostics(&self) -> &[CompatibilityDiagnostic] {
        &self.diagnostics
    }

    /// Returns whether the report contains no diagnostics.
    pub fn is_empty(&self) -> bool {
        self.diagnostics.is_empty()
    }

    /// Returns the number of diagnostic occurrences without coalescing.
    pub fn len(&self) -> usize {
        self.diagnostics.len()
    }
}

impl Default for CompatibilityReport {
    fn default() -> Self {
        Self::empty()
    }
}

/// One complete renderer-owned static export.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Artifact {
    pub(crate) format: OutputFormat,
    pub(crate) bytes: Vec<u8>,
}

impl Artifact {
    /// Returns the artifact's output format.
    pub const fn format(&self) -> OutputFormat {
        self.format
    }

    /// Returns the canonical media type for the artifact format.
    pub const fn media_type(&self) -> &'static str {
        self.format.media_type()
    }

    /// Borrows the complete artifact bytes.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Consumes the artifact and returns its complete owned bytes.
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

/// Successful result of one complete render transaction.
#[derive(Clone, Debug)]
pub struct RenderOutcome {
    pub(crate) artifact: Artifact,
    pub(crate) report: CompatibilityReport,
    pub(crate) profile: &'static ReferenceProfileIdentity,
}

impl RenderOutcome {
    /// Returns the single complete artifact.
    pub const fn artifact(&self) -> &Artifact {
        &self.artifact
    }

    /// Returns the complete ordered compatibility report.
    pub const fn report(&self) -> &CompatibilityReport {
        &self.report
    }

    /// Consumes the outcome without discarding either complete owned value.
    pub fn into_parts(self) -> (Artifact, CompatibilityReport) {
        (self.artifact, self.report)
    }

    /// Returns the reference profile the artifact reproduces: the scene profile
    /// for a scene export, the frame profile for a frame export.
    #[doc(hidden)]
    pub const fn profile_identity(&self) -> &'static ReferenceProfileIdentity {
        self.profile
    }
}

/// Stable category of a fatal render failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum RenderErrorCategory {
    /// Admitted scene input could not produce bounded global semantics.
    Input,
    /// A capability required by the render is unavailable in this build.
    Capability,
    /// A configured semantic resource ceiling was crossed.
    ResourceLimit,
    /// Rendering could not retain a meaningful complete artifact.
    NoMeaningfulArtifact,
    /// Complete artifact encoding failed.
    Encoding,
    /// A trusted renderer invariant failed.
    Internal,
}

impl RenderErrorCategory {
    /// Returns the stable serialized spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Input => "input",
            Self::Capability => "capability",
            Self::ResourceLimit => "resource_limit",
            Self::NoMeaningfulArtifact => "no_meaningful_artifact",
            Self::Encoding => "encoding",
            Self::Internal => "internal",
        }
    }
}

impl fmt::Display for RenderErrorCategory {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Fatal render failure with the complete report accumulated before failure.
///
/// [`Error::source`] is always `None`: dependency failures (decoders,
/// encoders, font tooling) are flattened into the stable code and message.
#[derive(Clone, Debug)]
pub struct RenderError {
    pub(crate) category: RenderErrorCategory,
    pub(crate) code: ErrorCode,
    pub(crate) message: Box<str>,
    pub(crate) format: OutputFormat,
    pub(crate) element_id: Option<String>,
    pub(crate) json_pointer: Box<str>,
    pub(crate) details: Option<Box<DiagnosticDetails>>,
    pub(crate) report: CompatibilityReport,
}

impl RenderError {
    /// Returns the stable fatal failure category.
    pub const fn category(&self) -> RenderErrorCategory {
        self.category
    }

    /// Returns the stable machine-readable error code.
    pub const fn code(&self) -> ErrorCode {
        self.code
    }

    /// Returns the selected output format.
    pub const fn format(&self) -> OutputFormat {
        self.format
    }

    /// Returns the normalized source element ID, when applicable.
    pub fn element_id(&self) -> Option<&str> {
        self.element_id.as_deref()
    }

    /// Returns the required RFC 6901 scene pointer.
    pub fn json_pointer(&self) -> &str {
        &self.json_pointer
    }

    /// Returns typed details required by the stable error code.
    pub fn details(&self) -> Option<&DiagnosticDetails> {
        self.details.as_deref()
    }

    /// Returns the complete ordered report accumulated before failure.
    pub const fn report(&self) -> &CompatibilityReport {
        &self.report
    }

    /// Returns this failure's stable serializable descriptor.
    pub fn descriptor(&self) -> ErrorDescriptor {
        ErrorDescriptor::render(
            self.category,
            self.code,
            self.to_string(),
            self.format,
            self.element_id.clone(),
            self.json_pointer.to_string(),
            self.details.as_deref().cloned(),
        )
    }
}

impl fmt::Display for RenderError {
    /// The failure's message; a limit crossing appends the measured value,
    /// the limit field and its configured ceiling.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)?;
        if let Some(crossing) = self.details().and_then(DiagnosticDetails::limit_crossing) {
            write!(formatter, ": {crossing}")?;
        }
        Ok(())
    }
}

impl Error for RenderError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic_registry::{ImageFailure, ImageRecovery, LocalRecovery};
    use crate::{Renderer, ResourceMeasurement, ResourceValue, ValueKind};

    /// A diagnostic retained first, unowned, in a scene export.
    fn retained_first(pending: PendingDiagnostic) -> CompatibilityDiagnostic {
        pending.retained(DiagnosticOrder::retained(
            None,
            None,
            0,
            DiagnosticAttribution::SceneGlobal,
        ))
    }

    #[test]
    fn owned_outcome_and_render_failure_expose_only_complete_contract_values() {
        let diagnostic = retained_first(
            PendingDiagnostic::new(
                DiagnosticKind::MalformedElement(LocalRecovery::Placeholder),
                OutputFormat::Svg,
                &[DiagnosticEffect::Visual],
                Some("element-1"),
                "/elements/0",
                "malformed element replaced",
            )
            .expect("the diagnostic allocates"),
        );
        let report = CompatibilityReport {
            schema_version: 1,
            diagnostics: vec![diagnostic],
        };
        let profile = Renderer::new().profile_identity();
        let outcome = RenderOutcome {
            artifact: Artifact {
                format: OutputFormat::Svg,
                bytes: b"<svg/>".to_vec(),
            },
            report: report.clone(),
            profile,
        };

        assert_eq!(outcome.artifact().bytes(), b"<svg/>");
        assert_eq!(outcome.artifact().media_type(), "image/svg+xml");
        assert_eq!(outcome.report(), &report);
        assert!(std::ptr::eq(outcome.profile_identity(), profile));
        assert_eq!(
            serde_json::to_string(outcome.report()).expect("serialize diagnosed report"),
            concat!(
                "{\"schema_version\":1,\"diagnostics\":[{",
                "\"code\":\"compatibility.malformed-element\",",
                "\"severity\":\"warning\",",
                "\"format\":\"svg\",",
                "\"effects\":[\"visual\"],",
                "\"element_id\":\"element-1\",",
                "\"json_pointer\":\"/elements/0\",",
                "\"message\":\"malformed element replaced\",",
                "\"details\":{\"kind\":\"compatibility\",",
                "\"recovery\":\"placeholder\",",
                "\"recipe_id\":\"renderer.malformed-element.v1\"}}]}"
            )
        );
        let (artifact, returned_report) = outcome.into_parts();
        assert_eq!(artifact.into_bytes(), b"<svg/>");
        assert_eq!(returned_report, report);

        let error = RenderError {
            category: RenderErrorCategory::ResourceLimit,
            code: ErrorCode::RESOURCE_OUTPUT_BYTES_EXCEEDED,
            message: "output byte limit exceeded".into(),
            format: OutputFormat::Svg,
            element_id: None,
            json_pointer: Box::from(""),
            details: Some(Box::new(DiagnosticDetails::ResourceLimit {
                field: "max_output_bytes",
                configured: ResourceValue::Integer(134_217_728),
                value_kind: ValueKind::Bytes,
                measurement: ResourceMeasurement::Attempted {
                    value: ResourceValue::Integer(134_217_729),
                },
                recovery: None,
            })),
            report: returned_report,
        };
        assert_eq!(error.report().len(), 1);
        assert_eq!(error.category(), RenderErrorCategory::ResourceLimit);
        assert_eq!(
            serde_json::to_string(&error.descriptor()).expect("serialize render descriptor"),
            concat!(
                "{\"family\":\"render\",",
                "\"category\":\"resource_limit\",",
                "\"code\":\"resource.output-bytes-exceeded\",",
                "\"message\":\"output byte limit exceeded: needs 134217729 bytes, ",
                "but max_output_bytes is 134217728\",",
                "\"format\":\"svg\",",
                "\"element_id\":null,",
                "\"json_pointer\":\"\",",
                "\"details\":{\"kind\":\"resource_limit\",",
                "\"field\":\"max_output_bytes\",",
                "\"configured\":134217728,",
                "\"value_kind\":\"bytes\",",
                "\"measurement\":{\"kind\":\"attempted\",\"value\":134217729},",
                "\"recovery\":null}}"
            )
        );
    }

    #[test]
    fn report_preserves_every_diagnostic_in_supplied_canonical_order() {
        let diagnostic = |kind, pointer: &str| {
            retained_first(
                PendingDiagnostic::new(
                    kind,
                    OutputFormat::Png,
                    &[DiagnosticEffect::Visual],
                    None,
                    pointer,
                    "diagnosed",
                )
                .expect("the diagnostic allocates"),
            )
        };
        let report = CompatibilityReport {
            schema_version: 1,
            diagnostics: vec![
                diagnostic(DiagnosticKind::InvalidColor, "/elements/0/strokeColor"),
                diagnostic(
                    DiagnosticKind::ImageUnavailable {
                        failure: ImageFailure::DecodeFailed {
                            media_type: "image/png",
                        },
                        asset_id: "image",
                        recovery: ImageRecovery::for_format(OutputFormat::Png, false),
                    },
                    "/files/image/dataURL",
                ),
            ],
        };

        assert_eq!(report.len(), 2);
        assert_eq!(
            report.diagnostics()[0].code(),
            DiagnosticCode::COLOR_INVALID
        );
        assert_eq!(
            report.diagnostics()[1].code(),
            DiagnosticCode::IMAGE_DECODE_FAILED
        );
    }
}
