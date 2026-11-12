use crate::{
    DiagnosticDetails, ErrorCode, Measurement, OutputFormat, Recovery, ResourceLimits,
    ResourceMeasurement, ResourceValue, ValueKind,
};
#[cfg(test)]
use crate::{ReferenceProfileIdentity, RenderRequest, SceneDocument};
use serde::Serialize;
use std::collections::TryReserveError;

const fn is_zero(value: &u64) -> bool {
    *value == 0
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
pub(crate) struct ResourceDelta {
    pub(crate) scene_bytes: u64,
    pub(crate) elements: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub(crate) asset_entries: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub(crate) asset_bytes: u64,
    #[serde(skip_serializing_if = "is_zero")]
    #[serde(rename = "relationship_depth")]
    pub(crate) observed_relationship_depth: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub(crate) geometry_segments: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub(crate) text_scalars: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub(crate) rasterized_pixels: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub(crate) decoded_pixels: u64,
    pub(crate) diagnostics: u64,
    pub(crate) working_bytes: u64,
    pub(crate) output_bytes: u64,
    pub(crate) total_work_units: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
struct ResourceCounters {
    scene_bytes: u64,
    elements: u64,
    #[serde(skip_serializing_if = "is_zero")]
    asset_entries: u64,
    #[serde(skip_serializing_if = "is_zero")]
    asset_bytes: u64,
    #[serde(skip_serializing_if = "is_zero")]
    relationship_depth: u64,
    #[serde(skip_serializing_if = "is_zero")]
    geometry_segments: u64,
    #[serde(skip_serializing_if = "is_zero")]
    text_scalars: u64,
    #[serde(skip_serializing_if = "is_zero")]
    rasterized_pixels: u64,
    #[serde(skip_serializing_if = "is_zero")]
    decoded_pixels: u64,
    diagnostics: u64,
    working_bytes: u64,
    output_bytes: u64,
    total_work_units: u64,
}

#[derive(Clone, Copy)]
pub(crate) struct ResourceContext<'a> {
    phase: ResourcePhase,
    event: &'static str,
    json_pointer: &'a str,
    element_id: Option<&'a str>,
    semantic_identity: Option<&'a str>,
    local_owner: Option<&'a str>,
    sharing_key: Option<&'a str>,
    record_kind: Option<ResourceRecordKind>,
    fallback_parent: Option<u64>,
}

pub(crate) trait ResourceAccount {
    type Error;

    fn transact_resource(
        &mut self,
        context: ResourceContext<'_>,
        delta: ResourceDelta,
    ) -> Result<(), Self::Error>;

    fn release_resource(
        &mut self,
        context: ResourceContext<'_>,
        bytes: u64,
    ) -> Result<(), Self::Error>;

    fn internal_failure(&mut self, message: &'static str) -> Self::Error;
}

impl<'a> ResourceContext<'a> {
    pub(crate) const fn new(
        phase: ResourcePhase,
        event: &'static str,
        json_pointer: &'a str,
    ) -> Self {
        Self {
            phase,
            event,
            json_pointer,
            element_id: None,
            semantic_identity: None,
            local_owner: None,
            sharing_key: None,
            record_kind: None,
            fallback_parent: None,
        }
    }

    pub(crate) const fn with_element_id(mut self, element_id: Option<&'a str>) -> Self {
        self.element_id = element_id;
        self
    }

    pub(crate) const fn with_local_owner(mut self, local_owner: Option<&'a str>) -> Self {
        self.local_owner = local_owner;
        self
    }

    pub(crate) const fn with_record(mut self, record_kind: ResourceRecordKind) -> Self {
        self.record_kind = Some(record_kind);
        self
    }

    pub(crate) const fn with_semantic_identity(mut self, identity: &'a str) -> Self {
        self.semantic_identity = Some(identity);
        self
    }

    pub(crate) const fn with_sharing_key(mut self, sharing_key: &'a str) -> Self {
        self.sharing_key = Some(sharing_key);
        self
    }

    pub(crate) fn try_failure_location(self) -> Result<ResourceFailureLocation, TryReserveError> {
        Ok(ResourceFailureLocation {
            json_pointer: try_owned(self.json_pointer)?,
            element_id: self.element_id.map(try_owned).transpose()?,
        })
    }
}

pub(crate) struct ResourceFailureLocation {
    json_pointer: String,
    element_id: Option<String>,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ResourcePhase {
    Admission,
    JsonTraversalAndNormalization,
    AuthorityReferenceCensus,
    IntrinsicAssets,
    SharedSemanticResolution,
    FormatPlanning,
    #[cfg(any(feature = "png", feature = "pdf", test))]
    ProjectionAndFallback,
    Metadata,
    EncodingAndOutcomeAssembly,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ResourceRecordKind {
    RequestSessionMeter,
    FormatPlanBase,
    FormatPlanOperation,
    BuiltInSelectionState,
    BuiltInGlyphOrRangeState,
    RequestDerivedBuiltInBlob,
    #[cfg(any(feature = "png", feature = "pdf"))]
    RasterColorSurface,
    #[cfg(feature = "png")]
    RasterMaskSurface,
    #[cfg(feature = "pdf")]
    PdfImageRgbFormatResource,
    #[cfg(feature = "pdf")]
    PdfImageAlphaFormatResource,
    Diagnostic,
    SvgSerializerWorkspace,
    SvgFontSubsetWorkspace,
    #[cfg(feature = "png")]
    PngEncoderWorkspace,
    #[cfg(feature = "pdf")]
    PdfStreamWorkspace,
    OutputBuffer,
}

pub(crate) struct ResourceMeter {
    limits: ResourceLimits,
    counters: ResourceCounters,
    trace: Option<ResourceTrace>,
}

pub(crate) struct OptionalMetadataCrossing {
    pub(crate) field: &'static str,
    pub(crate) configured: u64,
    pub(crate) value_kind: ValueKind,
    pub(crate) measurement: Measurement<u64>,
}

impl ResourceMeter {
    pub(crate) fn new(limits: &ResourceLimits) -> Self {
        Self {
            limits: limits.clone(),
            counters: ResourceCounters::default(),
            trace: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_trace(
        profile: &ReferenceProfileIdentity,
        scene: &SceneDocument,
        request: &RenderRequest,
    ) -> Self {
        Self {
            limits: request.resource_limits().clone(),
            counters: ResourceCounters::default(),
            trace: Some(ResourceTrace::new(profile, scene, request)),
        }
    }

    pub(crate) fn transact(
        &mut self,
        context: ResourceContext<'_>,
        delta: ResourceDelta,
    ) -> Result<(), ResourceCrossing> {
        self.transact_with_decision(context, delta, TraceDecision::Committed)
    }

    pub(crate) fn commit_local_rejection_observation(
        &mut self,
        context: ResourceContext<'_>,
        delta: ResourceDelta,
    ) -> Result<(), ResourceCrossing> {
        self.transact_with_decision(context, delta, TraceDecision::RejectedLocal)
    }

    pub(crate) fn preflight_local_rejection(
        &mut self,
        context: ResourceContext<'_>,
        delta: ResourceDelta,
    ) -> Result<(), ResourceCrossing> {
        let before = self.counters;
        let attempted = self.attempted(delta);
        if let Some((kind, crossing_attempt)) = fatal_crossing(attempted, &self.limits) {
            let measurement = crossing_attempt.measurement(kind.configured(&self.limits));
            self.record_transaction(
                context,
                delta,
                before,
                attempted_counters_for_trace(attempted, &self.limits),
                before,
                measurement_form(measurement),
                TraceDecision::RejectedFatal,
                Some(kind.code()),
            );
            return Err(ResourceCrossing { kind, measurement });
        }
        let attempted = attempted_counters_for_trace(attempted, &self.limits);
        self.record_transaction(
            context,
            delta,
            before,
            attempted,
            before,
            TraceMeasurementForm::Exact,
            TraceDecision::RejectedLocal,
            None,
        );
        Ok(())
    }

    fn transact_with_decision(
        &mut self,
        context: ResourceContext<'_>,
        delta: ResourceDelta,
        committed_decision: TraceDecision,
    ) -> Result<(), ResourceCrossing> {
        let before = self.counters;
        let attempted = self.attempted(delta);

        let crossing = fatal_crossing(attempted, &self.limits);

        if let Some((kind, crossing_attempt)) = crossing {
            let measurement = crossing_attempt.measurement(kind.configured(&self.limits));
            self.record_transaction(
                context,
                delta,
                before,
                attempted_counters_for_trace(attempted, &self.limits),
                before,
                measurement_form(measurement),
                TraceDecision::RejectedFatal,
                Some(kind.code()),
            );
            return Err(ResourceCrossing { kind, measurement });
        }

        if let Some(committed) = attempted.committed() {
            self.counters = committed;
            self.record_transaction(
                context,
                delta,
                before,
                committed,
                committed,
                TraceMeasurementForm::Exact,
                committed_decision,
                None,
            );
            Ok(())
        } else {
            let measurement = Measurement::AtLeast {
                value: self.limits.max_total_work_units().saturating_add(1),
            };
            self.record_transaction(
                context,
                delta,
                before,
                attempted_counters_for_trace(attempted, &self.limits),
                before,
                TraceMeasurementForm::AtLeast,
                TraceDecision::RejectedFatal,
                Some(ErrorCode::RESOURCE_TOTAL_WORK_EXCEEDED),
            );
            Err(ResourceCrossing {
                kind: LimitKind::TotalWork,
                measurement,
            })
        }
    }

    pub(crate) fn would_cross_fatal(&self, delta: ResourceDelta) -> bool {
        fatal_crossing(self.attempted(delta), &self.limits).is_some()
    }

    pub(crate) fn optional_metadata_crossing(
        &mut self,
        context: ResourceContext<'_>,
        delta: ResourceDelta,
    ) -> Option<OptionalMetadataCrossing> {
        let (kind, attempted) = fatal_crossing(self.attempted(delta), &self.limits)?;
        let configured = kind.configured(&self.limits);
        let measurement = attempted.measurement(configured);
        let before = self.counters;
        self.record_transaction(
            context,
            delta,
            before,
            attempted_counters_for_trace(self.attempted(delta), &self.limits),
            before,
            measurement_form(measurement),
            TraceDecision::RejectedLocal,
            None,
        );
        Some(OptionalMetadataCrossing {
            field: kind.field(),
            configured,
            value_kind: kind.value_kind(),
            measurement,
        })
    }

    fn attempted(&self, delta: ResourceDelta) -> AttemptedCounters {
        AttemptedCounters {
            scene_bytes: Attempted::new(self.counters.scene_bytes, delta.scene_bytes),
            elements: Attempted::new(self.counters.elements, delta.elements),
            asset_entries: Attempted::new(self.counters.asset_entries, delta.asset_entries),
            asset_bytes: Attempted::new(self.counters.asset_bytes, delta.asset_bytes),
            relationship_depth: Attempted::observed(
                self.counters.relationship_depth,
                delta.observed_relationship_depth,
            ),
            geometry_segments: Attempted::new(
                self.counters.geometry_segments,
                delta.geometry_segments,
            ),
            text_scalars: Attempted::new(self.counters.text_scalars, delta.text_scalars),
            rasterized_pixels: Attempted::new(
                self.counters.rasterized_pixels,
                delta.rasterized_pixels,
            ),
            decoded_pixels: Attempted::new(self.counters.decoded_pixels, delta.decoded_pixels),
            diagnostics: Attempted::new(self.counters.diagnostics, delta.diagnostics),
            working_bytes: Attempted::new(self.counters.working_bytes, delta.working_bytes),
            output_bytes: Attempted::new(self.counters.output_bytes, delta.output_bytes),
            total_work_units: Attempted::new(
                self.counters.total_work_units,
                delta.total_work_units,
            ),
        }
    }

    pub(crate) fn release_working_bytes(
        &mut self,
        context: ResourceContext<'_>,
        bytes: u64,
    ) -> Result<(), ()> {
        let before = self.counters;
        let mut after = before;
        after.working_bytes = after.working_bytes.checked_sub(bytes).ok_or(())?;
        self.counters = after;
        self.record_release(context, bytes, before, after);
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn record_transaction(
        &mut self,
        context: ResourceContext<'_>,
        delta: ResourceDelta,
        before: ResourceCounters,
        attempted: ResourceCounters,
        after: ResourceCounters,
        measurement: TraceMeasurementForm,
        decision: TraceDecision,
        code: Option<ErrorCode>,
    ) {
        let Some(trace) = &mut self.trace else {
            return;
        };
        let source_ordinal = trace.next_ordinal();
        trace.push(ResourceTraceEntry::transaction(
            source_ordinal,
            context,
            delta,
            before,
            attempted,
            after,
            measurement,
            decision,
            code,
        ));
    }

    fn record_release(
        &mut self,
        context: ResourceContext<'_>,
        bytes: u64,
        before: ResourceCounters,
        after: ResourceCounters,
    ) {
        let Some(trace) = &mut self.trace else {
            return;
        };
        let source_ordinal = trace.next_ordinal();
        trace.push(ResourceTraceEntry::release(
            source_ordinal,
            context,
            bytes,
            before,
            after,
        ));
    }

    #[cfg(test)]
    pub(crate) fn trace_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(
            self.trace
                .as_ref()
                .expect("test trace is enabled for this render session"),
        )
        .expect("resource trace contains only serializable values")
    }
}

#[derive(Clone, Copy)]
struct AttemptedCounters {
    scene_bytes: Attempted,
    elements: Attempted,
    asset_entries: Attempted,
    asset_bytes: Attempted,
    relationship_depth: Attempted,
    geometry_segments: Attempted,
    text_scalars: Attempted,
    rasterized_pixels: Attempted,
    decoded_pixels: Attempted,
    diagnostics: Attempted,
    working_bytes: Attempted,
    output_bytes: Attempted,
    total_work_units: Attempted,
}

impl AttemptedCounters {
    fn committed(self) -> Option<ResourceCounters> {
        Some(ResourceCounters {
            scene_bytes: self.scene_bytes.exact()?,
            elements: self.elements.exact()?,
            asset_entries: self.asset_entries.exact()?,
            asset_bytes: self.asset_bytes.exact()?,
            relationship_depth: self.relationship_depth.exact()?,
            geometry_segments: self.geometry_segments.exact()?,
            text_scalars: self.text_scalars.exact()?,
            rasterized_pixels: self.rasterized_pixels.exact()?,
            decoded_pixels: self.decoded_pixels.exact()?,
            diagnostics: self.diagnostics.exact()?,
            working_bytes: self.working_bytes.exact()?,
            output_bytes: self.output_bytes.exact()?,
            total_work_units: self.total_work_units.exact()?,
        })
    }
}

fn fatal_crossing(
    attempted: AttemptedCounters,
    limits: &ResourceLimits,
) -> Option<(LimitKind, Attempted)> {
    [
        (LimitKind::SceneBytes, attempted.scene_bytes),
        (LimitKind::Elements, attempted.elements),
        (LimitKind::AssetEntries, attempted.asset_entries),
        (LimitKind::TotalAssetBytes, attempted.asset_bytes),
        (LimitKind::RelationshipDepth, attempted.relationship_depth),
        (LimitKind::TotalGeometry, attempted.geometry_segments),
        (LimitKind::TotalText, attempted.text_scalars),
        (LimitKind::TotalDecodedPixels, attempted.decoded_pixels),
        (
            LimitKind::TotalRasterizedPixels,
            attempted.rasterized_pixels,
        ),
        (LimitKind::Diagnostics, attempted.diagnostics),
        (LimitKind::WorkingBytes, attempted.working_bytes),
        (LimitKind::OutputBytes, attempted.output_bytes),
        (LimitKind::TotalWork, attempted.total_work_units),
    ]
    .into_iter()
    .find(|(kind, attempted)| attempted.crosses(kind.configured(limits)))
}

fn attempted_counters_for_trace(
    attempted: AttemptedCounters,
    limits: &ResourceLimits,
) -> ResourceCounters {
    ResourceCounters {
        scene_bytes: attempted.scene_bytes.trace_value(limits.max_scene_bytes()),
        elements: attempted
            .elements
            .trace_value(u64::from(limits.max_elements())),
        asset_entries: attempted
            .asset_entries
            .trace_value(u64::from(limits.max_asset_entries())),
        asset_bytes: attempted
            .asset_bytes
            .trace_value(limits.max_total_asset_bytes()),
        relationship_depth: attempted
            .relationship_depth
            .trace_value(u64::from(limits.max_relationship_depth())),
        geometry_segments: attempted
            .geometry_segments
            .trace_value(limits.max_total_geometry_segments()),
        text_scalars: attempted
            .text_scalars
            .trace_value(limits.max_total_text_scalars()),
        rasterized_pixels: attempted
            .rasterized_pixels
            .trace_value(limits.max_total_rasterized_pixels()),
        decoded_pixels: attempted
            .decoded_pixels
            .trace_value(limits.max_total_decoded_pixels()),
        diagnostics: attempted
            .diagnostics
            .trace_value(u64::from(limits.max_diagnostics())),
        working_bytes: attempted
            .working_bytes
            .trace_value(limits.max_working_bytes()),
        output_bytes: attempted
            .output_bytes
            .trace_value(limits.max_output_bytes()),
        total_work_units: attempted
            .total_work_units
            .trace_value(limits.max_total_work_units()),
    }
}

const fn measurement_form(measurement: Measurement<u64>) -> TraceMeasurementForm {
    match measurement {
        Measurement::Attempted { .. } => TraceMeasurementForm::Exact,
        Measurement::AtLeast { .. } => TraceMeasurementForm::AtLeast,
    }
}

#[derive(Clone, Copy)]
enum Attempted {
    Exact(u64),
    Overflow,
}

impl Attempted {
    fn new(current: u64, delta: u64) -> Self {
        // Deltas that cannot be represented exactly use `u64::MAX` as the
        // transaction-local overflow sentinel.
        if delta == u64::MAX {
            return Self::Overflow;
        }
        current
            .checked_add(delta)
            .map_or(Self::Overflow, Self::Exact)
    }

    const fn observed(current: u64, value: u64) -> Self {
        Self::Exact(if current > value { current } else { value })
    }

    fn crosses(self, configured: u64) -> bool {
        match self {
            Self::Exact(value) => value > configured,
            Self::Overflow => true,
        }
    }

    fn exact(self) -> Option<u64> {
        match self {
            Self::Exact(value) => Some(value),
            Self::Overflow => None,
        }
    }

    fn measurement(self, configured: u64) -> Measurement<u64> {
        match self {
            Self::Exact(value) => Measurement::Attempted { value },
            Self::Overflow => Measurement::AtLeast {
                value: configured.saturating_add(1),
            },
        }
    }

    fn trace_value(self, configured: u64) -> u64 {
        match self {
            Self::Exact(value) => value,
            Self::Overflow => configured.saturating_add(1),
        }
    }
}

#[derive(Clone, Copy)]
enum LimitKind {
    SceneBytes,
    Elements,
    AssetEntries,
    TotalAssetBytes,
    RelationshipDepth,
    TotalGeometry,
    TotalText,
    TotalRasterizedPixels,
    TotalDecodedPixels,
    Diagnostics,
    WorkingBytes,
    OutputBytes,
    TotalWork,
}

impl LimitKind {
    fn configured(self, limits: &ResourceLimits) -> u64 {
        match self {
            Self::SceneBytes => limits.max_scene_bytes(),
            Self::Elements => u64::from(limits.max_elements()),
            Self::AssetEntries => u64::from(limits.max_asset_entries()),
            Self::TotalAssetBytes => limits.max_total_asset_bytes(),
            Self::RelationshipDepth => u64::from(limits.max_relationship_depth()),
            Self::TotalGeometry => limits.max_total_geometry_segments(),
            Self::TotalText => limits.max_total_text_scalars(),
            Self::TotalRasterizedPixels => limits.max_total_rasterized_pixels(),
            Self::TotalDecodedPixels => limits.max_total_decoded_pixels(),
            Self::Diagnostics => u64::from(limits.max_diagnostics()),
            Self::WorkingBytes => limits.max_working_bytes(),
            Self::OutputBytes => limits.max_output_bytes(),
            Self::TotalWork => limits.max_total_work_units(),
        }
    }

    const fn code(self) -> ErrorCode {
        self.metadata().code
    }

    const fn field(self) -> &'static str {
        self.metadata().field
    }

    const fn value_kind(self) -> ValueKind {
        self.metadata().value_kind
    }

    const fn message(self) -> &'static str {
        self.metadata().message
    }

    const fn metadata(self) -> LimitMetadata {
        match self {
            Self::SceneBytes => LimitMetadata {
                code: ErrorCode::RESOURCE_SCENE_BYTES_EXCEEDED,
                field: "max_scene_bytes",
                value_kind: ValueKind::Bytes,
                message: "scene byte limit exceeded",
            },
            Self::Elements => LimitMetadata {
                code: ErrorCode::RESOURCE_ELEMENT_COUNT_EXCEEDED,
                field: "max_elements",
                value_kind: ValueKind::Count,
                message: "element count limit exceeded",
            },
            Self::AssetEntries => LimitMetadata {
                code: ErrorCode::RESOURCE_ASSET_COUNT_EXCEEDED,
                field: "max_asset_entries",
                value_kind: ValueKind::Count,
                message: "asset payload occurrence limit exceeded",
            },
            Self::TotalAssetBytes => LimitMetadata {
                code: ErrorCode::RESOURCE_TOTAL_ASSET_BYTES_EXCEEDED,
                field: "max_total_asset_bytes",
                value_kind: ValueKind::Bytes,
                message: "total asset byte limit exceeded",
            },
            Self::RelationshipDepth => LimitMetadata {
                code: ErrorCode::RESOURCE_RELATIONSHIP_DEPTH_EXCEEDED,
                field: "max_relationship_depth",
                value_kind: ValueKind::Depth,
                message: "relationship traversal depth limit exceeded",
            },
            Self::TotalGeometry => LimitMetadata {
                code: ErrorCode::RESOURCE_TOTAL_GEOMETRY_EXCEEDED,
                field: "max_total_geometry_segments",
                value_kind: ValueKind::Count,
                message: "total geometry segment limit exceeded",
            },
            Self::TotalText => LimitMetadata {
                code: ErrorCode::RESOURCE_TOTAL_TEXT_EXCEEDED,
                field: "max_total_text_scalars",
                value_kind: ValueKind::Count,
                message: "total text scalar limit exceeded",
            },
            Self::TotalRasterizedPixels => LimitMetadata {
                code: ErrorCode::RESOURCE_TOTAL_RASTERIZED_PIXELS_EXCEEDED,
                field: "max_total_rasterized_pixels",
                value_kind: ValueKind::Pixels,
                message: "total rasterized pixel limit exceeded",
            },
            Self::TotalDecodedPixels => LimitMetadata {
                code: ErrorCode::RESOURCE_TOTAL_DECODED_PIXELS_EXCEEDED,
                field: "max_total_decoded_pixels",
                value_kind: ValueKind::Pixels,
                message: "total decoded pixel limit exceeded",
            },
            Self::Diagnostics => LimitMetadata {
                code: ErrorCode::RESOURCE_DIAGNOSTIC_COUNT_EXCEEDED,
                field: "max_diagnostics",
                value_kind: ValueKind::Count,
                message: "diagnostic count limit exceeded",
            },
            Self::WorkingBytes => LimitMetadata {
                code: ErrorCode::RESOURCE_WORKING_BYTES_EXCEEDED,
                field: "max_working_bytes",
                value_kind: ValueKind::Bytes,
                message: "working byte limit exceeded",
            },
            Self::OutputBytes => LimitMetadata {
                code: ErrorCode::RESOURCE_OUTPUT_BYTES_EXCEEDED,
                field: "max_output_bytes",
                value_kind: ValueKind::Bytes,
                message: "output byte limit exceeded",
            },
            Self::TotalWork => LimitMetadata {
                code: ErrorCode::RESOURCE_TOTAL_WORK_EXCEEDED,
                field: "max_total_work_units",
                value_kind: ValueKind::WorkUnits,
                message: "total work limit exceeded",
            },
        }
    }
}

struct LimitMetadata {
    code: ErrorCode,
    field: &'static str,
    value_kind: ValueKind,
    message: &'static str,
}

pub(crate) struct ResourceCrossing {
    kind: LimitKind,
    measurement: Measurement<u64>,
}

impl ResourceCrossing {
    fn details(&self, limits: &ResourceLimits) -> DiagnosticDetails {
        let measurement: ResourceMeasurement = match self.measurement {
            Measurement::Attempted { value } => Measurement::Attempted {
                value: ResourceValue::Integer(value),
            },
            Measurement::AtLeast { value } => Measurement::AtLeast {
                value: ResourceValue::Integer(value),
            },
        };
        DiagnosticDetails::ResourceLimit {
            field: self.kind.field(),
            configured: ResourceValue::Integer(self.kind.configured(limits)),
            value_kind: self.kind.value_kind(),
            measurement,
            recovery: None::<Recovery>,
        }
    }

    pub(crate) fn into_failure_parts(
        self,
        limits: &ResourceLimits,
        location: ResourceFailureLocation,
    ) -> (
        ErrorCode,
        &'static str,
        Option<String>,
        String,
        DiagnosticDetails,
    ) {
        let details = self.details(limits);
        (
            self.kind.code(),
            self.kind.message(),
            location.element_id,
            location.json_pointer,
            details,
        )
    }
}

fn try_owned(value: &str) -> Result<String, TryReserveError> {
    let mut owned = String::new();
    owned.try_reserve_exact(value.len())?;
    owned.push_str(value);
    Ok(owned)
}

#[derive(Serialize)]
struct ResourceTrace {
    schema_version: u32,
    header: ResourceTraceHeader,
    entries: Vec<ResourceTraceEntry>,
}

impl ResourceTrace {
    #[cfg(test)]
    fn new(
        profile: &ReferenceProfileIdentity,
        scene: &SceneDocument,
        request: &RenderRequest,
    ) -> Self {
        let supplemental_assets = request
            .supplemental_assets()
            .iter()
            .map(|(file_id, asset)| SupplementalAssetDigest {
                file_id: file_id.to_owned(),
                payload_sha256: sha256_string(&sha256_digest(asset.bytes())),
            })
            .collect();
        Self {
            schema_version: 1,
            header: ResourceTraceHeader {
                compatibility_revision: profile.compatibility_revision().get(),
                reference_profile_digest: profile.reference_profile_digest().to_owned(),
                format: request.format(),
                limits: ResourceTraceLimits::from(request.resource_limits()),
                input_digests: ResourceInputDigests {
                    scene_document_sha256: sha256_string(scene.source_sha256()),
                    supplemental_assets,
                },
            },
            entries: Vec::new(),
        }
    }

    fn next_ordinal(&self) -> u64 {
        u64::try_from(self.entries.len()).unwrap_or(u64::MAX)
    }

    fn push(&mut self, entry: ResourceTraceEntry) {
        self.entries.push(entry);
    }
}

#[derive(Serialize)]
struct ResourceTraceHeader {
    compatibility_revision: u64,
    reference_profile_digest: String,
    format: OutputFormat,
    limits: ResourceTraceLimits,
    input_digests: ResourceInputDigests,
}

#[derive(Serialize)]
struct ResourceInputDigests {
    scene_document_sha256: String,
    supplemental_assets: Vec<SupplementalAssetDigest>,
}

#[derive(Serialize)]
struct SupplementalAssetDigest {
    file_id: String,
    payload_sha256: String,
}

#[derive(Serialize)]
struct ResourceTraceLimits {
    max_scene_bytes: u64,
    max_elements: u32,
    max_asset_entries: u32,
    max_asset_bytes: u64,
    max_total_asset_bytes: u64,
    max_relationship_depth: u32,
    max_nested_asset_depth: u32,
    max_nested_svg_nodes: u32,
    max_geometry_segments_per_element: u32,
    max_total_geometry_segments: u64,
    max_text_scalars_per_element: u32,
    max_total_text_scalars: u64,
    max_raster_dimension: u32,
    max_decoded_pixels_per_asset: u64,
    max_total_decoded_pixels: u64,
    max_raster_surface_pixels: u64,
    max_total_rasterized_pixels: u64,
    max_svg_dimension: f64,
    max_pdf_page_points: f64,
    max_coordinate_magnitude: f64,
    max_diagnostics: u32,
    max_working_bytes: u64,
    max_output_bytes: u64,
    max_element_work_units: u64,
    max_asset_work_units: u64,
    max_total_work_units: u64,
}

impl From<&ResourceLimits> for ResourceTraceLimits {
    fn from(limits: &ResourceLimits) -> Self {
        Self {
            max_scene_bytes: limits.max_scene_bytes(),
            max_elements: limits.max_elements(),
            max_asset_entries: limits.max_asset_entries(),
            max_asset_bytes: limits.max_asset_bytes(),
            max_total_asset_bytes: limits.max_total_asset_bytes(),
            max_relationship_depth: limits.max_relationship_depth(),
            max_nested_asset_depth: limits.max_nested_asset_depth(),
            max_nested_svg_nodes: limits.max_nested_svg_nodes(),
            max_geometry_segments_per_element: limits.max_geometry_segments_per_element(),
            max_total_geometry_segments: limits.max_total_geometry_segments(),
            max_text_scalars_per_element: limits.max_text_scalars_per_element(),
            max_total_text_scalars: limits.max_total_text_scalars(),
            max_raster_dimension: limits.max_raster_dimension(),
            max_decoded_pixels_per_asset: limits.max_decoded_pixels_per_asset(),
            max_total_decoded_pixels: limits.max_total_decoded_pixels(),
            max_raster_surface_pixels: limits.max_raster_surface_pixels(),
            max_total_rasterized_pixels: limits.max_total_rasterized_pixels(),
            max_svg_dimension: limits.max_svg_dimension(),
            max_pdf_page_points: limits.max_pdf_page_points(),
            max_coordinate_magnitude: limits.max_coordinate_magnitude(),
            max_diagnostics: limits.max_diagnostics(),
            max_working_bytes: limits.max_working_bytes(),
            max_output_bytes: limits.max_output_bytes(),
            max_element_work_units: limits.max_element_work_units(),
            max_asset_work_units: limits.max_asset_work_units(),
            max_total_work_units: limits.max_total_work_units(),
        }
    }
}

#[derive(Serialize)]
struct ResourceTraceEntry {
    phase: ResourcePhase,
    source_ordinal: u64,
    json_pointer: String,
    element_id: Option<String>,
    semantic_identity: Option<String>,
    event: &'static str,
    local_owner: Option<String>,
    sharing_key: Option<String>,
    record_kind: Option<ResourceRecordKind>,
    deltas: ResourceDelta,
    before: ResourceCounters,
    attempted: ResourceCounters,
    after: ResourceCounters,
    measurement: TraceMeasurementForm,
    decision: TraceDecision,
    code: Option<ErrorCode>,
    fallback_parent: Option<u64>,
    record_action: Option<TraceRecordAction>,
}

impl ResourceTraceEntry {
    #[allow(clippy::too_many_arguments)]
    fn transaction(
        source_ordinal: u64,
        context: ResourceContext<'_>,
        deltas: ResourceDelta,
        before: ResourceCounters,
        attempted: ResourceCounters,
        after: ResourceCounters,
        measurement: TraceMeasurementForm,
        decision: TraceDecision,
        code: Option<ErrorCode>,
    ) -> Self {
        Self {
            phase: context.phase,
            source_ordinal,
            json_pointer: context.json_pointer.to_owned(),
            element_id: context.element_id.map(str::to_owned),
            semantic_identity: context.semantic_identity.map(str::to_owned),
            event: context.event,
            local_owner: context.local_owner.map(str::to_owned),
            sharing_key: context.sharing_key.map(str::to_owned),
            record_kind: context.record_kind,
            deltas,
            before,
            attempted,
            after,
            measurement,
            decision,
            code,
            fallback_parent: context.fallback_parent,
            record_action: context.record_kind.map(|_| TraceRecordAction::Acquire),
        }
    }

    fn release(
        source_ordinal: u64,
        context: ResourceContext<'_>,
        working_bytes: u64,
        before: ResourceCounters,
        after: ResourceCounters,
    ) -> Self {
        Self {
            phase: context.phase,
            source_ordinal,
            json_pointer: context.json_pointer.to_owned(),
            element_id: context.element_id.map(str::to_owned),
            semantic_identity: context.semantic_identity.map(str::to_owned),
            event: context.event,
            local_owner: context.local_owner.map(str::to_owned),
            sharing_key: context.sharing_key.map(str::to_owned),
            record_kind: context.record_kind,
            deltas: ResourceDelta {
                working_bytes,
                ..ResourceDelta::default()
            },
            before,
            attempted: after,
            after,
            measurement: TraceMeasurementForm::Exact,
            decision: TraceDecision::Released,
            code: None,
            fallback_parent: context.fallback_parent,
            record_action: Some(TraceRecordAction::Release),
        }
    }
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum TraceMeasurementForm {
    Exact,
    AtLeast,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum TraceDecision {
    Committed,
    RejectedLocal,
    RejectedFatal,
    /// One of the five decisions the spec admits in a canonical trace
    /// (§13, "Decisions are only `committed`, `rejected_local`,
    /// `rejected_fatal`, `shared_intrinsic`, and `released`"). Revision 1
    /// records no transaction with it; the variant keeps the serialized
    /// vocabulary complete.
    #[allow(dead_code)]
    SharedIntrinsic,
    Released,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum TraceRecordAction {
    Acquire,
    Release,
}

#[cfg(test)]
pub(crate) fn sha256_digest(input: &[u8]) -> [u8; 32] {
    use sha2::Digest;
    sha2::Sha256::digest(input).into()
}

#[cfg(test)]
pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    lower_hex(&sha256_digest(bytes))
}

#[cfg(test)]
fn sha256_string(digest: &[u8]) -> String {
    let mut output = String::from("sha256:");
    output.push_str(&lower_hex(digest));
    output
}

#[cfg(test)]
pub(crate) fn lower_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trace_sha256_matches_standard_vectors() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn unrepresentable_delta_is_an_at_least_crossing() {
        let limits = ResourceLimits::default();
        let mut meter = ResourceMeter::new(&limits);
        let context = ResourceContext::new(ResourcePhase::Admission, "overflow_vector", "");
        let location = context
            .try_failure_location()
            .expect("empty failure location needs no allocation");

        let crossing = meter
            .transact(
                context,
                ResourceDelta {
                    total_work_units: u64::MAX,
                    ..ResourceDelta::default()
                },
            )
            .expect_err("overflow sentinel crosses total work");
        let (code, _, _, _, details) = crossing.into_failure_parts(&limits, location);

        assert_eq!(code, ErrorCode::RESOURCE_TOTAL_WORK_EXCEEDED);
        match details {
            DiagnosticDetails::ResourceLimit {
                measurement:
                    Measurement::AtLeast {
                        value: ResourceValue::Integer(value),
                    },
                ..
            } => assert_eq!(value, limits.max_total_work_units() + 1),
            _ => panic!("overflow uses an at-least resource measurement"),
        }
    }
}
