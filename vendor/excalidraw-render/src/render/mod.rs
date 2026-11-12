mod accounting;
mod asset;
#[cfg(test)]
pub(crate) use asset::{with_cms_failure, with_cms_failure_after, CmsSite};
#[cfg(test)]
pub(crate) fn stage4_render_trace(
    scene: &crate::SceneDocument,
    request: &crate::RenderRequest,
) -> (Result<crate::RenderOutcome, crate::RenderError>, Vec<u8>) {
    render_with_trace(crate::catalog::REVISION_1.profile(), scene, request)
}
mod bound_text;
mod diagnostics;
#[cfg(not(test))]
use diagnostics::sort_diagnostics;
#[cfg(test)]
pub(crate) use diagnostics::sort_diagnostics;
mod encode;
mod font_demand;
mod font_subset;
#[cfg(test)]
mod font_subset_fuzz;
mod font_subset_layout;
#[cfg(test)]
mod font_subset_layout_fixtures;
#[cfg(test)]
mod font_subset_layout_tests;
// The SVG font subset viewer qualification manifest pins this module's digest
// (`tests/conformance/svg-font-subset-viewer-qualification/revision-1.json`),
// so it only changes with a re-qualification.
#[cfg(test)]
mod font_subset_qualification;
#[cfg(all(test, target_arch = "wasm32"))]
pub(crate) use font_subset_fuzz::{
    assert_font_subset_fuzz_regression_corpus, assert_font_subset_fuzz_smoke,
};
#[cfg(all(test, target_arch = "wasm32"))]
pub(crate) use font_subset_qualification::assert_complete_font_subset_qualification_corpus;
mod coordinates;
#[cfg(test)]
mod frame_batch_property;
mod frame_target;
mod freedraw;
mod geometry;
mod image;
mod metadata;
#[cfg(test)]
pub(crate) use metadata::{test_faults, Stage as MetadataStage};
mod notices;
mod number_format;
#[cfg(feature = "pdf")]
mod pdf;
mod plan;
#[cfg(test)]
mod plan_tests;
#[cfg(feature = "png")]
mod png;
#[cfg(any(feature = "png", feature = "pdf"))]
mod png_encoder;
mod prepared;
pub(crate) use prepared::{FramePreparationError, PreparedFrameRender};
#[cfg(test)]
mod resolve_tests;
#[cfg(test)]
mod resource_trace_qualification;
#[cfg(all(test, target_arch = "wasm32"))]
pub(crate) use resource_trace_qualification::assert_font_subset_resource_trace_matrix;
#[cfg(test)]
mod release_order_tests;
mod resolution;
mod resolve;
mod rough;
mod svg;
#[cfg(any(feature = "png", feature = "pdf"))]
mod svg_filter_work;
mod svg_fonts;
#[cfg(test)]
mod test_support;
#[cfg(all(test, any(feature = "png", feature = "pdf")))]
use test_support::resolve_with_svg_trees_for_test;
#[cfg(test)]
use test_support::{
    font_subset_cache_stats, font_subset_qualification_error, new_font_subset_cache,
    prepared_font_subset_cache_stats, render_font_subset_qualification_trace,
    render_frame_with_trace, render_with_trace, render_with_trace_and_font_subset_cache,
    resolve_for_test,
};
#[cfg(test)]
mod tests;
mod text;

use std::sync::{Arc, Mutex};

use crate::catalog::RevisionCatalog;
use crate::diagnostic_registry::MetadataFailure;
use crate::outcome::{DiagnosticAttribution, DiagnosticOrder, PendingDiagnostic};
use crate::resource_accounting::{
    ResourceAccount, ResourceContext, ResourceCrossing, ResourceDelta, ResourceMeter,
    ResourcePhase, ResourceRecordKind,
};
use crate::scene::OriginalJsonVisitError;
use crate::{
    Artifact, CompatibilityDiagnostic, CompatibilityReport, DiagnosticDetails, ErrorCode,
    FiniteValue, Measurement, OutputFormat, ReferenceProfileIdentity, RenderError,
    RenderErrorCategory, RenderOutcome, RenderRequest, ResourceValue, SceneDocument, Sizing,
    ValueKind,
};

use self::frame_target::FrameTarget;
use self::resolve::{logical_aligned_bytes, ResolvedScene, SemanticWorkingRecord};

#[derive(Clone, Copy, Debug)]
pub(super) struct FontUnavailable {
    resolved_family: &'static str,
    asset_identity: &'static str,
}

impl FontUnavailable {
    fn face(face: &crate::fonts::Face) -> Self {
        Self {
            resolved_family: face.family().catalog_name(),
            asset_identity: face.identity(),
        }
    }
}

#[derive(Debug)]
pub(super) enum FormatEncodeError {
    Encoding(String),
    FontUnavailable(FontUnavailable),
    SvgFontSubset {
        stage: crate::SvgFontSubsetStage,
        resolved_family: &'static str,
        asset_identity: &'static str,
    },
    Metadata(MetadataFailure),
    MemoryUnavailable {
        subject_kind: &'static str,
        subject: &'static str,
    },
}

pub(crate) fn render(
    catalog: &'static RevisionCatalog,
    scene: &SceneDocument,
    request: &RenderRequest,
) -> Result<RenderOutcome, RenderError> {
    render_with_svg_font_payload(catalog, scene, request, svg_fonts::Payload::Subset)
}

fn render_with_svg_font_payload(
    catalog: &'static RevisionCatalog,
    scene: &SceneDocument,
    request: &RenderRequest,
    svg_font_payload: svg_fonts::Payload,
) -> Result<RenderOutcome, RenderError> {
    let profile = catalog.profile();
    debug_assert_eq!(
        catalog.asset_catalog().compatibility_revision,
        profile.compatibility_revision().get()
    );
    debug_assert_eq!(
        catalog.asset_catalog().reference_profile_digest,
        profile.reference_profile_digest()
    );
    let mut session = RenderSession::new(profile, catalog.scene_surface(), scene, request);
    session.render(scene, request, None, svg_font_payload)
}

pub(crate) fn render_frame(
    catalog: &'static RevisionCatalog,
    frame: crate::FrameRef<'_>,
    request: &crate::FrameRenderRequest,
) -> Result<RenderOutcome, RenderError> {
    render_frame_scoped(catalog, frame, request, None).0
}

pub(crate) fn render_frame_scoped(
    catalog: &'static RevisionCatalog,
    frame: crate::FrameRef<'_>,
    request: &crate::FrameRenderRequest,
    prepared: Option<&PreparedFrameRender>,
) -> (
    Result<RenderOutcome, RenderError>,
    Vec<CompatibilityDiagnostic>,
    Vec<CompatibilityDiagnostic>,
) {
    let profile = catalog.frame_profile();
    let target = FrameTarget {
        source_index: frame.source_index,
        ordinal: frame.ordinal(),
        kind: frame.kind(),
    };
    let request = request.as_render_request();
    let mut session = RenderSession::new_target(
        profile,
        catalog.scene_surface(),
        frame.scene,
        request,
        RenderTarget::Frame(target),
    );
    if let Some(prepared) = prepared {
        session.font_subset_session = Arc::clone(&prepared.font_subset_session);
    }
    let result = session.render(frame.scene, request, prepared, svg_fonts::Payload::Subset);
    let report = match &result {
        Ok(outcome) => outcome.report(),
        Err(error) => error.report(),
    };
    let global_diagnostics = report
        .diagnostics()
        .iter()
        .filter(|diagnostic| diagnostic.order().attribution == DiagnosticAttribution::SceneGlobal)
        .cloned()
        .collect();
    let target_diagnostics = report
        .diagnostics()
        .iter()
        .filter(|diagnostic| diagnostic.order().attribution == DiagnosticAttribution::Target)
        .cloned()
        .collect();
    (result, global_diagnostics, target_diagnostics)
}

#[derive(Clone, Copy)]
enum RenderTarget {
    Scene,
    Frame(FrameTarget),
}

type SharedFontSubsetSession = Arc<Mutex<svg_fonts::Session>>;
/// Sanitized-SVG viewer trees, keyed by asset identity. The sanitized bytes
/// are contract state and live in the resolved scene; the parsed tree is a
/// deterministic derivation of those bytes that only the raster-capable
/// backends consume, so it lives in the request-scoped session's caches, not
/// in the shared semantic graph (spec section 9).
type SvgTreeCache = std::collections::BTreeMap<[u8; 32], std::sync::Arc<usvg::Tree>>;

struct RenderSession {
    profile: &'static ReferenceProfileIdentity,
    scene_surface: &'static crate::scene_surface::SceneSurfacePolicy,
    report: CompatibilityReport,
    meter: ResourceMeter,
    staged_output: Option<Vec<u8>>,
    element_source_order_base: u64,
    excalifont_selected: bool,
    excalifont_range_selected: bool,
    excalifont_glyphs: Vec<u16>,
    excalifont_request_derived_bytes: u64,
    pending_png_animation_diagnostics: Vec<PendingAnimationDiagnostic>,
    pending_frame_normalization_diagnostics: Vec<PendingNormalizationDiagnostic>,
    target: RenderTarget,
    diagnostic_attribution: DiagnosticAttribution,
    font_subset_session: SharedFontSubsetSession,
    svg_trees: SvgTreeCache,
}

struct RenderResourceAccount<'a> {
    session: &'a mut RenderSession,
    request: &'a RenderRequest,
}

impl<'a> RenderResourceAccount<'a> {
    fn new(session: &'a mut RenderSession, request: &'a RenderRequest) -> Self {
        Self { session, request }
    }
}

impl ResourceAccount for RenderResourceAccount<'_> {
    type Error = RenderError;

    fn transact_resource(
        &mut self,
        context: ResourceContext<'_>,
        delta: ResourceDelta,
    ) -> Result<(), Self::Error> {
        self.session.transact(self.request, context, delta)
    }

    fn release_resource(
        &mut self,
        context: ResourceContext<'_>,
        bytes: u64,
    ) -> Result<(), Self::Error> {
        self.session.release_working(self.request, context, bytes)
    }

    fn internal_failure(&mut self, message: &'static str) -> Self::Error {
        self.session
            .internal_failure(self.request.format(), message, "")
    }
}

/// An animation diagnostic shared resolution queues for a PNG request; only
/// the PNG encoder flushes the queue, so a build without it reads no field.
#[cfg_attr(not(feature = "png"), allow(dead_code))]
struct PendingAnimationDiagnostic {
    diagnostic: PendingDiagnostic,
    local_owner: String,
    source_order: u64,
    working_bytes: u64,
}

struct PendingNormalizationDiagnostic {
    diagnostic: PendingDiagnostic,
    local_owner: Option<String>,
    source_order: u64,
    source_position: u64,
}

#[derive(Clone, Copy, Default)]
struct DiagnosticOwnership<'a> {
    local_owner: Option<&'a str>,
    source_order: Option<u64>,
    source_position: Option<u64>,
}

impl RenderSession {
    fn new(
        profile: &'static ReferenceProfileIdentity,
        scene_surface: &'static crate::scene_surface::SceneSurfacePolicy,
        scene: &SceneDocument,
        request: &RenderRequest,
    ) -> Self {
        Self::new_target(profile, scene_surface, scene, request, RenderTarget::Scene)
    }

    fn new_target(
        profile: &'static ReferenceProfileIdentity,
        scene_surface: &'static crate::scene_surface::SceneSurfacePolicy,
        scene: &SceneDocument,
        request: &RenderRequest,
        target: RenderTarget,
    ) -> Self {
        Self {
            profile,
            scene_surface,
            report: CompatibilityReport::empty(),
            meter: ResourceMeter::new(request.resource_limits()),
            staged_output: None,
            element_source_order_base: scene.source_owner_order("elements", 0),
            excalifont_selected: false,
            excalifont_range_selected: false,
            excalifont_glyphs: Vec::new(),
            excalifont_request_derived_bytes: 0,
            pending_png_animation_diagnostics: Vec::new(),
            pending_frame_normalization_diagnostics: Vec::new(),
            target,
            diagnostic_attribution: DiagnosticAttribution::SceneGlobal,
            font_subset_session: Arc::new(Mutex::new(svg_fonts::Session::new())),
            svg_trees: SvgTreeCache::new(),
        }
    }

    fn render(
        &mut self,
        scene: &SceneDocument,
        request: &RenderRequest,
        prepared: Option<&PreparedFrameRender>,
        svg_font_payload: svg_fonts::Payload,
    ) -> Result<RenderOutcome, RenderError> {
        self.admit_session(scene, request, prepared)?;
        let mut semantic_working = Vec::new();
        let mut resolved = self.resolve_scene(scene, request, prepared, &mut semantic_working)?;
        self.account_fallback_semantics(&mut resolved, request, &mut semantic_working)?;
        let bytes = self.encode(
            resolved,
            scene,
            request,
            &semantic_working,
            svg_font_payload,
        )?;

        Ok(RenderOutcome {
            artifact: Artifact {
                format: request.format(),
                bytes,
            },
            report: self.take_report(),
            profile: self.profile,
        })
    }

    /// Charges the fixed per-request session cost and replays or performs the
    /// original-JSON traversal that seeds semantic accounting.
    fn admit_session(
        &mut self,
        scene: &SceneDocument,
        request: &RenderRequest,
        prepared: Option<&PreparedFrameRender>,
    ) -> Result<(), RenderError> {
        self.transact(
            request,
            ResourceContext::new(ResourcePhase::Admission, "request_session", "")
                .with_record(ResourceRecordKind::RequestSessionMeter),
            ResourceDelta {
                working_bytes: 4_096,
                ..ResourceDelta::default()
            },
        )?;
        self.transact(
            request,
            ResourceContext::new(ResourcePhase::Admission, "scene_source_octets", ""),
            ResourceDelta {
                scene_bytes: scene.source_bytes(),
                total_work_units: scene.source_bytes(),
                ..ResourceDelta::default()
            },
        )?;
        if let Some(prepared) = prepared {
            for event in &prepared.original_json_events {
                self.observe_original_json_event(
                    request,
                    event.kind,
                    &event.json_pointer,
                    event.is_element_occurrence,
                )?;
            }
        } else {
            let traversal = scene.visit_original_json_entries(|entry| {
                self.observe_original_json_event(
                    request,
                    entry.kind,
                    entry.json_pointer,
                    entry.is_element_occurrence,
                )
            });
            match traversal {
                Ok(()) => {}
                Err(OriginalJsonVisitError::Visitor(error)) => return Err(error),
                Err(OriginalJsonVisitError::MemoryUnavailable { json_pointer }) => {
                    return Err(self.memory_failure_at(
                        request,
                        "semantic_traversal",
                        "scene_json_pointer",
                        None,
                        json_pointer,
                    ));
                }
            }
        }
        Ok(())
    }

    fn encode(
        &mut self,
        resolved: ResolvedScene,
        scene: &SceneDocument,
        request: &RenderRequest,
        semantic_working: &[SemanticWorkingRecord],
        svg_font_payload: svg_fonts::Payload,
    ) -> Result<Vec<u8>, RenderError> {
        match request.format() {
            OutputFormat::Svg => self.validate_svg_dimensions(&resolved, request)?,
            #[cfg(feature = "pdf")]
            OutputFormat::Pdf => self.validate_pdf_dimensions(&resolved, request)?,
            _ => {}
        }
        let plan_base = ResourceContext::new(ResourcePhase::FormatPlanning, "format_plan_base", "")
            .with_record(ResourceRecordKind::FormatPlanBase);
        self.acquire_working(request, plan_base, 1_024)?;
        let pdf_profile_selection = if request.format() == OutputFormat::Pdf {
            let selection = ResourceContext::new(
                ResourcePhase::FormatPlanning,
                "pdf_srgb_profile_selection",
                "",
            )
            .with_record(ResourceRecordKind::BuiltInSelectionState);
            self.acquire_working(request, selection, 256)?;
            Some(selection)
        } else {
            None
        };
        let operation_bytes = if resolved.include_background { 128 } else { 0 };
        let background_operation =
            ResourceContext::new(ResourcePhase::FormatPlanning, "background_operation", "")
                .with_record(ResourceRecordKind::FormatPlanOperation);
        if operation_bytes != 0 {
            self.transact(
                request,
                background_operation,
                ResourceDelta {
                    working_bytes: operation_bytes,
                    total_work_units: 8,
                    ..ResourceDelta::default()
                },
            )?;
        }
        for index in 0..resolved.source_count() {
            let (element_id, json_pointer) = resolved.source(index).ok_or_else(|| {
                self.internal_failure(
                    request.format(),
                    "resolved source operation is missing",
                    "/elements",
                )
            })?;
            self.transact(
                request,
                ResourceContext::new(
                    ResourcePhase::FormatPlanning,
                    "element_operation",
                    json_pointer,
                )
                .with_element_id(element_id)
                .with_local_owner(element_id)
                .with_record(ResourceRecordKind::FormatPlanOperation),
                ResourceDelta {
                    working_bytes: 128,
                    total_work_units: 8,
                    ..ResourceDelta::default()
                },
            )?;
        }
        match request.format() {
            OutputFormat::Svg => {
                self.encode_svg(resolved, scene, request, semantic_working, svg_font_payload)?;
            }
            #[cfg(feature = "png")]
            OutputFormat::Png => self.encode_png(resolved, scene, request, semantic_working)?,
            #[cfg(feature = "pdf")]
            OutputFormat::Pdf => self.encode_pdf(resolved, scene, request, semantic_working)?,
            // A request for a format this build leaves out never builds
            // (`request.format-unavailable`), so no render reaches this arm.
            #[cfg(not(all(feature = "png", feature = "pdf")))]
            _ => {
                return Err(self.internal_failure(
                    request.format(),
                    "the requested output format is not enabled in this build",
                    "",
                ));
            }
        }

        if let Some(profile_selection) = pdf_profile_selection {
            self.release_working(request, profile_selection, 256)?;
        }
        if operation_bytes != 0 {
            self.release_working(request, background_operation, operation_bytes)?;
        }
        self.release_working(request, plan_base, 1_024)?;

        self.staged_output.take().ok_or_else(|| {
            self.internal_failure(request.format(), "accepted output bytes are missing", "")
        })
    }

    /// The main PNG surface's crossing. `attempted` is `None` when a side
    /// saturated: the area is then reported as the checked-overflow lower
    /// bound `configured + 1` (spec sections 11.1 and 13.2), not the
    /// saturated product.
    #[cfg(feature = "png")]
    fn required_raster_surface_failure(
        &mut self,
        request: &RenderRequest,
        configured: u64,
        attempted: Option<u64>,
    ) -> RenderError {
        let measurement = match attempted {
            Some(attempted) => Measurement::Attempted {
                value: ResourceValue::Integer(attempted),
            },
            None => Measurement::AtLeast {
                value: ResourceValue::Integer(configured.saturating_add(1)),
            },
        };
        RenderError {
            category: RenderErrorCategory::ResourceLimit,
            code: ErrorCode::RESOURCE_RASTER_SURFACE_EXCEEDED,
            message: Box::from("required PNG raster surface exceeds its configured limit"),
            format: request.format(),
            element_id: None,
            json_pointer: Box::from(""),
            details: Some(Box::new(DiagnosticDetails::ResourceLimit {
                field: "max_raster_surface_pixels",
                configured: ResourceValue::Integer(configured),
                value_kind: ValueKind::Pixels,
                measurement,
                recovery: None,
            })),
            report: self.take_report(),
        }
    }

    fn charge_local_omission(
        &mut self,
        request: &RenderRequest,
        element_id: Option<&str>,
        json_pointer: &str,
        event: &'static str,
    ) -> Result<(), RenderError> {
        self.charge_work(
            request,
            ResourceContext::new(ResourcePhase::SharedSemanticResolution, event, json_pointer)
                .with_element_id(element_id)
                .with_local_owner(element_id),
            8,
        )
    }

    fn acquire_working(
        &mut self,
        request: &RenderRequest,
        context: ResourceContext<'_>,
        working_bytes: u64,
    ) -> Result<(), RenderError> {
        self.transact(
            request,
            context,
            ResourceDelta {
                working_bytes,
                ..ResourceDelta::default()
            },
        )
    }

    fn charge_work(
        &mut self,
        request: &RenderRequest,
        context: ResourceContext<'_>,
        total_work_units: u64,
    ) -> Result<(), RenderError> {
        self.transact(
            request,
            context,
            ResourceDelta {
                total_work_units,
                ..ResourceDelta::default()
            },
        )
    }

    #[cfg(test)]
    fn retain_diagnostic(
        &mut self,
        request: &RenderRequest,
        phase: ResourcePhase,
        event: &'static str,
        diagnostic: PendingDiagnostic,
    ) -> Result<(), RenderError> {
        self.retain_diagnostic_with_owner(
            request,
            phase,
            event,
            diagnostic,
            DiagnosticOwnership::default(),
        )
    }

    #[cfg(feature = "png")]
    fn flush_pending_png_animation_diagnostics(
        &mut self,
        request: &RenderRequest,
    ) -> Result<(), RenderError> {
        let pending = std::mem::take(&mut self.pending_png_animation_diagnostics);
        for pending in &pending {
            self.release_pending_png_animation_diagnostic(request, pending)?;
        }
        for pending in pending {
            self.retain_diagnostic_with_owner(
                request,
                ResourcePhase::ProjectionAndFallback,
                "image_animation_flattened",
                pending.diagnostic,
                DiagnosticOwnership {
                    local_owner: Some(&pending.local_owner),
                    source_order: Some(pending.source_order),
                    source_position: Some(u64::MAX),
                },
            )?;
        }
        Ok(())
    }

    #[cfg(feature = "png")]
    fn release_pending_png_animation_diagnostic(
        &mut self,
        request: &RenderRequest,
        pending: &PendingAnimationDiagnostic,
    ) -> Result<(), RenderError> {
        self.release_working(
            request,
            ResourceContext::new(
                ResourcePhase::SharedSemanticResolution,
                "pending_image_animation_flattened",
                pending.diagnostic.json_pointer(),
            )
            .with_element_id(pending.diagnostic.element_id())
            .with_local_owner(Some(&pending.local_owner)),
            pending.working_bytes,
        )
    }

    fn retain_diagnostic_with_owner(
        &mut self,
        request: &RenderRequest,
        phase: ResourcePhase,
        event: &'static str,
        diagnostic: PendingDiagnostic,
        ownership: DiagnosticOwnership<'_>,
    ) -> Result<(), RenderError> {
        let pointer_bytes = u64::try_from(diagnostic.json_pointer().len()).unwrap_or(u64::MAX);
        let element_id_bytes = diagnostic.element_id().map_or(0, |element_id| {
            u64::try_from(element_id.len()).unwrap_or(u64::MAX)
        });
        let record_bytes = 512_u64
            .saturating_add(logical_aligned_bytes(pointer_bytes))
            .saturating_add(logical_aligned_bytes(element_id_bytes));
        self.transact(
            request,
            ResourceContext::new(phase, event, diagnostic.json_pointer())
                .with_element_id(diagnostic.element_id())
                .with_local_owner(ownership.local_owner.or(diagnostic.element_id()))
                .with_record(ResourceRecordKind::Diagnostic),
            ResourceDelta {
                diagnostics: 1,
                working_bytes: record_bytes,
                total_work_units: 8,
                ..ResourceDelta::default()
            },
        )?;
        if self.report.diagnostics.try_reserve(1).is_err() {
            let (element_id, json_pointer) = diagnostic.into_location();
            return Err(self.memory_failure_at(
                request,
                "compatibility_report",
                "diagnostic_occurrence",
                element_id,
                json_pointer,
            ));
        }
        let order = DiagnosticOrder::retained(
            ownership.source_order.or_else(|| {
                element_owner_index(diagnostic.json_pointer())
                    .map(|index| self.element_source_order_base.saturating_add(index))
            }),
            ownership.source_position,
            u64::try_from(self.report.diagnostics.len()).unwrap_or(u64::MAX),
            self.diagnostic_attribution,
        );
        self.report.diagnostics.push(diagnostic.retained(order));
        Ok(())
    }

    fn scope_frame_diagnostics(&mut self, participating: &[usize]) {
        self.report.diagnostics.retain_mut(|diagnostic| {
            let Some(source_index) = element_owner_index(diagnostic.json_pointer()) else {
                return true;
            };
            let Ok(source_index) = usize::try_from(source_index) else {
                return false;
            };
            if participating.binary_search(&source_index).is_err() {
                return false;
            }
            diagnostic.attribute_to_target();
            true
        });
    }

    fn flush_frame_normalization_diagnostics(
        &mut self,
        request: &RenderRequest,
        participating: &[usize],
        selected_file_ids: &[String],
    ) -> Result<(), RenderError> {
        let pending = std::mem::take(&mut self.pending_frame_normalization_diagnostics);
        for pending in pending {
            let element_source = element_owner_index(pending.diagnostic.json_pointer())
                .and_then(|source_index| usize::try_from(source_index).ok());
            if element_source
                .is_some_and(|source_index| participating.binary_search(&source_index).is_err())
            {
                continue;
            }
            if pending.diagnostic.json_pointer().starts_with("/files/")
                && !selected_file_ids.iter().any(|file_id| {
                    crate::scene::pointer_child("/files", file_id).is_ok_and(|pointer| {
                        pending.diagnostic.json_pointer().starts_with(&pointer)
                    })
                })
            {
                continue;
            }
            self.diagnostic_attribution = if element_source.is_some()
                || pending.diagnostic.json_pointer().starts_with("/files/")
            {
                DiagnosticAttribution::Target
            } else {
                DiagnosticAttribution::SceneGlobal
            };
            self.retain_diagnostic_with_owner(
                request,
                ResourcePhase::JsonTraversalAndNormalization,
                "normalization_repair",
                pending.diagnostic,
                DiagnosticOwnership {
                    local_owner: pending.local_owner.as_deref(),
                    source_order: Some(pending.source_order),
                    source_position: Some(pending.source_position),
                },
            )?;
        }
        Ok(())
    }

    fn take_report(&mut self) -> CompatibilityReport {
        sort_diagnostics(&mut self.report.diagnostics);
        std::mem::take(&mut self.report)
    }

    #[cfg(any(feature = "png", feature = "pdf", test))]
    fn emit_premeasured_output(
        &mut self,
        request: &RenderRequest,
        error_code: ErrorCode,
        codec_emission_work: u64,
        measured: Vec<u8>,
        mut encode: impl FnMut() -> Result<Vec<u8>, FormatEncodeError>,
    ) -> Result<(), RenderError> {
        let output_bytes = u64::try_from(measured.len()).unwrap_or(u64::MAX);
        self.admit_output(request, output_bytes, codec_emission_work)?;

        let emitted = encode().map_err(|error| self.format_failure(request, error_code, error))?;
        if emitted != measured {
            return Err(self.internal_failure(
                request.format(),
                "measured and emitted artifact bytes differ",
                "",
            ));
        }
        self.staged_output = Some(emitted);
        Ok(())
    }

    fn admit_output(
        &mut self,
        request: &RenderRequest,
        output_bytes: u64,
        codec_emission_work: u64,
    ) -> Result<(), RenderError> {
        let output_record_bytes = 512_u64.saturating_add(logical_aligned_bytes(output_bytes));
        let total_work_units = codec_emission_work.saturating_add(output_bytes);
        self.transact(
            request,
            ResourceContext::new(
                ResourcePhase::EncodingAndOutcomeAssembly,
                "artifact_retention",
                "",
            )
            .with_record(ResourceRecordKind::OutputBuffer),
            ResourceDelta {
                working_bytes: output_record_bytes,
                output_bytes,
                total_work_units,
                ..ResourceDelta::default()
            },
        )?;
        Ok(())
    }

    fn release_semantic_working(
        &mut self,
        request: &RenderRequest,
        records: &[SemanticWorkingRecord],
        asset_records: bool,
    ) -> Result<(), RenderError> {
        for record in records {
            let is_asset = asset::is_persistent_event(record.event);
            if is_asset != asset_records {
                continue;
            }
            self.release_working(
                request,
                ResourceContext::new(
                    ResourcePhase::FormatPlanning,
                    record.event,
                    &record.json_pointer,
                )
                .with_element_id(record.element_id.as_deref())
                .with_local_owner(record.local_owner.as_deref()),
                record.bytes,
            )?;
        }
        Ok(())
    }

    fn release_working(
        &mut self,
        request: &RenderRequest,
        context: ResourceContext<'_>,
        bytes: u64,
    ) -> Result<(), RenderError> {
        self.meter
            .release_working_bytes(context, bytes)
            .map_err(|()| {
                self.internal_failure(
                    request.format(),
                    "semantic working-byte record underflow",
                    "",
                )
            })
    }

    fn transact(
        &mut self,
        request: &RenderRequest,
        context: ResourceContext<'_>,
        delta: ResourceDelta,
    ) -> Result<(), RenderError> {
        let failure_location = match context.try_failure_location() {
            Ok(location) => location,
            Err(_) => {
                return Err(self.memory_failure(request, "resource_failure", "source_location"));
            }
        };
        match self.meter.transact(context, delta) {
            Ok(()) => Ok(()),
            Err(crossing) => Err(self.resource_failure(request, crossing, failure_location)),
        }
    }

    fn commit_local_rejection_observation(
        &mut self,
        request: &RenderRequest,
        context: ResourceContext<'_>,
        delta: ResourceDelta,
    ) -> Result<(), RenderError> {
        let failure_location = match context.try_failure_location() {
            Ok(location) => location,
            Err(_) => {
                return Err(self.memory_failure(request, "resource_failure", "source_location"));
            }
        };
        match self
            .meter
            .commit_local_rejection_observation(context, delta)
        {
            Ok(()) => Ok(()),
            Err(crossing) => Err(self.resource_failure(request, crossing, failure_location)),
        }
    }

    fn preflight_local_rejection(
        &mut self,
        request: &RenderRequest,
        context: ResourceContext<'_>,
        delta: ResourceDelta,
    ) -> Result<(), RenderError> {
        let failure_location = context
            .try_failure_location()
            .map_err(|_| self.memory_failure(request, "resource_failure", "source_location"))?;
        match self.meter.preflight_local_rejection(context, delta) {
            Ok(()) => Ok(()),
            Err(crossing) => Err(self.resource_failure(request, crossing, failure_location)),
        }
    }

    fn resource_failure(
        &mut self,
        request: &RenderRequest,
        crossing: ResourceCrossing,
        location: crate::resource_accounting::ResourceFailureLocation,
    ) -> RenderError {
        let (code, message, element_id, json_pointer, details) =
            crossing.into_failure_parts(request.resource_limits(), location);
        RenderError {
            category: RenderErrorCategory::ResourceLimit,
            code,
            message: Box::from(message),
            format: request.format(),
            element_id,
            json_pointer: json_pointer.into_boxed_str(),
            details: Some(Box::new(details)),
            report: self.take_report(),
        }
    }

    fn validate_svg_dimensions(
        &mut self,
        resolved: &ResolvedScene,
        request: &RenderRequest,
    ) -> Result<(), RenderError> {
        let scale = match request.sizing() {
            Sizing::Scale(scale) => scale,
            Sizing::MaxDimension(_) => 1.0,
        };
        let limit = request.resource_limits().max_svg_dimension();
        for attempted in [
            resolved.logical_width,
            resolved.logical_height,
            resolved.logical_width * scale,
            resolved.logical_height * scale,
        ] {
            if attempted > limit {
                let configured = FiniteValue::new(limit).ok_or_else(|| {
                    self.internal_failure(
                        request.format(),
                        "validated SVG dimension limit is not finite",
                        "",
                    )
                })?;
                // A scale large enough to overflow has no finite attempted value;
                // the crossing reports the finite lower bound instead.
                let measurement = match FiniteValue::new(attempted) {
                    Some(attempted) => Measurement::Attempted {
                        value: ResourceValue::Finite(attempted),
                    },
                    None => Measurement::AtLeast {
                        value: ResourceValue::Finite(
                            FiniteValue::new(limit.next_up()).unwrap_or(configured),
                        ),
                    },
                };
                return Err(RenderError {
                    category: RenderErrorCategory::ResourceLimit,
                    code: ErrorCode::RESOURCE_SVG_DIMENSIONS_EXCEEDED,
                    message: Box::from("SVG dimension limit exceeded"),
                    format: request.format(),
                    element_id: None,
                    json_pointer: Box::from(""),
                    details: Some(Box::new(DiagnosticDetails::ResourceLimit {
                        field: "max_svg_dimension",
                        configured: ResourceValue::Finite(configured),
                        value_kind: ValueKind::Dimension,
                        measurement,
                        recovery: None,
                    })),
                    report: self.take_report(),
                });
            }
        }
        Ok(())
    }

    #[cfg(feature = "pdf")]
    fn validate_pdf_dimensions(
        &mut self,
        resolved: &ResolvedScene,
        request: &RenderRequest,
    ) -> Result<(), RenderError> {
        let limit = request.resource_limits().max_pdf_page_points();
        let scale = resolved.raster_or_pdf_scale(request.sizing());
        for logical in [resolved.logical_width, resolved.logical_height] {
            let attempted = logical * scale * pdf::POINTS_PER_LOGICAL_PIXEL;
            if attempted > limit {
                let configured = FiniteValue::new(limit).ok_or_else(|| {
                    self.internal_failure(
                        request.format(),
                        "validated PDF page limit is not finite",
                        "",
                    )
                })?;
                let measurement = if let Some(attempted) = FiniteValue::new(attempted) {
                    Measurement::Attempted {
                        value: ResourceValue::Finite(attempted),
                    }
                } else {
                    let lower_bound = FiniteValue::new(limit + 1.0).ok_or_else(|| {
                        self.internal_failure(
                            request.format(),
                            "PDF page crossing lower bound is not finite",
                            "",
                        )
                    })?;
                    Measurement::AtLeast {
                        value: ResourceValue::Finite(lower_bound),
                    }
                };
                return Err(RenderError {
                    category: RenderErrorCategory::ResourceLimit,
                    code: ErrorCode::RESOURCE_PDF_PAGE_DIMENSIONS_EXCEEDED,
                    message: Box::from("PDF page point limit exceeded"),
                    format: request.format(),
                    element_id: None,
                    json_pointer: Box::from(""),
                    details: Some(Box::new(DiagnosticDetails::ResourceLimit {
                        field: "max_pdf_page_points",
                        configured: ResourceValue::Finite(configured),
                        value_kind: ValueKind::Dimension,
                        measurement,
                        recovery: None,
                    })),
                    report: self.take_report(),
                });
            }
        }
        Ok(())
    }

    fn format_failure(
        &mut self,
        request: &RenderRequest,
        code: ErrorCode,
        error: FormatEncodeError,
    ) -> RenderError {
        match error {
            FormatEncodeError::FontUnavailable(font) => {
                self.font_unavailable_failure(request.format(), font)
            }
            FormatEncodeError::Encoding(message) => {
                self.encoding_failure(request.format(), code, message)
            }
            FormatEncodeError::Metadata(_) => self.encoding_failure(
                request.format(),
                code,
                "mandatory artifact metadata could not be encoded".to_owned(),
            ),
            FormatEncodeError::SvgFontSubset {
                stage,
                resolved_family,
                asset_identity,
            } => RenderError {
                category: RenderErrorCategory::Encoding,
                code: ErrorCode::ENCODE_SVG_FONT_SUBSET_FAILED,
                message: Box::from("a required SVG font subset could not be produced"),
                format: request.format(),
                element_id: None,
                json_pointer: Box::from(""),
                details: Some(Box::new(DiagnosticDetails::SvgFontSubsetFailure {
                    stage,
                    resolved_family: resolved_family.to_owned(),
                    asset_identity: asset_identity.to_owned(),
                })),
                report: self.take_report(),
            },
            FormatEncodeError::MemoryUnavailable {
                subject_kind,
                subject,
            } => self.memory_failure(request, subject_kind, subject),
        }
    }

    fn memory_failure(
        &mut self,
        request: &RenderRequest,
        subject_kind: &str,
        subject: &str,
    ) -> RenderError {
        self.memory_failure_at(request, subject_kind, subject, None, String::new())
    }

    fn memory_failure_at(
        &mut self,
        request: &RenderRequest,
        subject_kind: &str,
        subject: &str,
        element_id: Option<String>,
        json_pointer: String,
    ) -> RenderError {
        RenderError {
            category: RenderErrorCategory::ResourceLimit,
            code: ErrorCode::RESOURCE_MEMORY_UNAVAILABLE,
            message: Box::from("required physical allocation is unavailable"),
            format: request.format(),
            element_id,
            json_pointer: json_pointer.into_boxed_str(),
            details: Some(Box::new(DiagnosticDetails::MemoryUnavailable {
                subject_kind: subject_kind.to_owned(),
                subject: subject.to_owned(),
            })),
            report: self.take_report(),
        }
    }

    fn encoding_failure(
        &mut self,
        format: OutputFormat,
        code: ErrorCode,
        message: String,
    ) -> RenderError {
        RenderError {
            category: RenderErrorCategory::Encoding,
            code,
            message: message.into_boxed_str(),
            format,
            element_id: None,
            json_pointer: Box::from(""),
            details: None,
            report: self.take_report(),
        }
    }

    fn font_unavailable_failure(
        &mut self,
        format: OutputFormat,
        font: FontUnavailable,
    ) -> RenderError {
        RenderError {
            category: RenderErrorCategory::Capability,
            code: ErrorCode::RENDER_FONT_UNAVAILABLE,
            // Xiaolai is the one optional font: the default `cjk` feature bundles it.
            message: Box::from(if font.resolved_family == "Xiaolai" {
                "the Xiaolai CJK font this text needs is not in this build; enable the `cjk` feature of excalidraw-render"
            } else {
                "a required bundled font is unavailable in this build"
            }),
            format,
            element_id: None,
            json_pointer: Box::from(""),
            details: Some(Box::new(DiagnosticDetails::FontUnavailable {
                resolved_family: font.resolved_family.to_owned(),
                asset_identity: font.asset_identity.to_owned(),
            })),
            report: self.take_report(),
        }
    }

    fn internal_failure(
        &mut self,
        format: OutputFormat,
        message: &'static str,
        json_pointer: &'static str,
    ) -> RenderError {
        RenderError {
            category: RenderErrorCategory::Internal,
            code: ErrorCode::RENDER_INTERNAL_FAILURE,
            message: Box::from(message),
            format,
            element_id: None,
            json_pointer: Box::from(json_pointer),
            details: None,
            report: self.take_report(),
        }
    }

    fn unbounded_geometry_failure(&mut self, format: OutputFormat) -> RenderError {
        RenderError {
            category: RenderErrorCategory::Input,
            code: ErrorCode::SCENE_UNBOUNDED_GEOMETRY,
            message: Box::from("finite whole-scene bounds could not be established"),
            format,
            element_id: None,
            json_pointer: Box::from(""),
            details: None,
            report: self.take_report(),
        }
    }
}

fn element_owner_index(json_pointer: &str) -> Option<u64> {
    json_pointer
        .strip_prefix("/elements/")?
        .split('/')
        .next()?
        .parse()
        .ok()
}

fn try_owned(value: &str) -> Result<String, std::collections::TryReserveError> {
    let mut owned = String::new();
    owned.try_reserve_exact(value.len())?;
    owned.push_str(value);
    Ok(owned)
}
