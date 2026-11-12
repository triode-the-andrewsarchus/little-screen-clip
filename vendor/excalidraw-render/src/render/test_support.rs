//! Test-only entry points into the session: renders and resolutions that
//! return the canonical resource trace beside the outcome, sessions that share
//! or inspect a font subset cache, and the resolve-level seam that stops before
//! planning and encoding. The sibling test modules reach these through
//! `render`; nothing here is compiled outside `cfg(test)`.

use std::sync::{Arc, Mutex};

use crate::catalog::{RevisionCatalog, REVISION_1};
use crate::outcome::DiagnosticAttribution;
use crate::resource_accounting::ResourceMeter;
use crate::scene_surface::SceneSurfacePolicy;
use crate::{
    CompatibilityReport, ErrorCode, FrameRef, FrameRenderRequest, OutputFormat,
    ReferenceProfileIdentity, RenderError, RenderOutcome, RenderRequest, Renderer, SceneDocument,
    SvgFontSubsetStage,
};

use super::font_demand::release_svg_font_encoding_records;
use super::frame_target::FrameTarget;
use super::prepared::PreparedFrameRender;
use super::resolve::ResolvedScene;
use super::{
    svg_fonts, FormatEncodeError, RenderResourceAccount, RenderSession, RenderTarget,
    SharedFontSubsetSession, SvgTreeCache,
};

pub(super) fn render_with_trace(
    profile: &'static ReferenceProfileIdentity,
    scene: &SceneDocument,
    request: &RenderRequest,
) -> (Result<RenderOutcome, RenderError>, Vec<u8>) {
    let mut session =
        RenderSession::new_with_trace(profile, REVISION_1.scene_surface(), scene, request);
    let result = session.render(scene, request, None, svg_fonts::Payload::Subset);
    (result, session.meter.trace_bytes())
}

/// The format-neutral semantic graph (spec §9) one render of `scene` under
/// `request` would encode: admission, resolution and fallback accounting run
/// exactly as `RenderSession::render` runs them, and the session stops before
/// planning and encoding. Tests assert pre-encode behavior here once instead
/// of once per format.
pub(super) fn resolve_for_test(
    scene: &SceneDocument,
    request: &RenderRequest,
) -> Result<ResolvedScene, RenderError> {
    resolve_with_svg_trees_for_test(scene, request).map(|(resolved, _)| resolved)
}

/// [`resolve_for_test`] together with the nested-SVG trees resolution parsed,
/// which `PngPlan::new` and `PdfPlan::new` take beside the resolved scene.
pub(super) fn resolve_with_svg_trees_for_test(
    scene: &SceneDocument,
    request: &RenderRequest,
) -> Result<(ResolvedScene, SvgTreeCache), RenderError> {
    let mut session = RenderSession::new(
        REVISION_1.profile(),
        REVISION_1.scene_surface(),
        scene,
        request,
    );
    session.admit_session(scene, request, None)?;
    let mut semantic_working = Vec::new();
    let mut resolved = session.resolve_scene(scene, request, None, &mut semantic_working)?;
    session.account_fallback_semantics(&mut resolved, request, &mut semantic_working)?;
    Ok((resolved, session.svg_trees))
}

/// The canonical resource trace of one SVG font demand for `family` (a
/// resolved family name, or `Noto Emoji` for the fallback face) over
/// `scalars`, run through the same accounting the SVG encoder runs.
pub(super) fn render_font_subset_qualification_trace(
    family: &'static str,
    scalars: &[u32],
) -> (Result<(), RenderError>, Vec<u8>) {
    let scene = SceneDocument::parse(
        br#"{"type":"excalidraw","version":2,"elements":[],"appState":{},"files":{}}"#,
    )
    .expect("qualification trace scene is valid");
    let request = RenderRequest::builder(OutputFormat::Svg)
        .build()
        .expect("qualification SVG request is valid");
    let profile = Renderer::new().profile_identity();
    let mut session =
        RenderSession::new_with_trace(profile, REVISION_1.scene_surface(), &scene, &request);
    let result = (|| {
        let selection = svg_fonts::account_demand(
            &mut RenderResourceAccount::new(&mut session, &request),
            std::iter::once((family, scalars.len())),
        )?;
        let demand = svg_fonts::Demand {
            families: vec![
                svg_fonts::FamilyDemand::from_scalars(family, scalars).map_err(|_| {
                    session.memory_failure(&request, "svg_font_demand", "family_scalar_set")
                })?,
            ],
        };
        let selection = svg_fonts::account_selection(
            &mut RenderResourceAccount::new(&mut session, &request),
            &demand,
            selection,
        )?;
        let envelope = svg_fonts::account_dependency_envelope(
            &mut RenderResourceAccount::new(&mut session, &request),
            &demand,
        )?;
        let mut subset_session = svg_fonts::Session::new();
        let bundle = svg_fonts::build(demand, svg_fonts::Payload::Subset, &mut subset_session)
            .map_err(|error| {
                session.format_failure(&request, ErrorCode::ENCODE_SVG_FAILED, error)
            })?;
        bundle
            .account_resource_transcript(&mut RenderResourceAccount::new(&mut session, &request))?;
        release_svg_font_encoding_records(&mut session, &request, None, envelope)?;
        selection.release(&mut RenderResourceAccount::new(&mut session, &request))
    })();
    (result, session.meter.trace_bytes())
}

pub(super) fn font_subset_qualification_error(
    stage: SvgFontSubsetStage,
    resolved_family: &'static str,
    asset_identity: &'static str,
) -> RenderError {
    let scene = SceneDocument::parse(
        br#"{"type":"excalidraw","version":2,"elements":[],"appState":{},"files":{}}"#,
    )
    .expect("qualification failure scene is valid");
    let request = RenderRequest::builder(OutputFormat::Svg)
        .build()
        .expect("qualification failure request is valid");
    let mut session = RenderSession::new(
        REVISION_1.profile(),
        REVISION_1.scene_surface(),
        &scene,
        &request,
    );
    session.format_failure(
        &request,
        ErrorCode::ENCODE_SVG_FAILED,
        FormatEncodeError::SvgFontSubset {
            stage,
            resolved_family,
            asset_identity,
        },
    )
}

pub(super) fn new_font_subset_cache() -> SharedFontSubsetSession {
    Arc::new(Mutex::new(svg_fonts::Session::new()))
}

pub(super) fn render_with_trace_and_font_subset_cache(
    profile: &'static ReferenceProfileIdentity,
    scene: &SceneDocument,
    request: &RenderRequest,
    cache: &SharedFontSubsetSession,
) -> (Result<RenderOutcome, RenderError>, Vec<u8>) {
    let mut session =
        RenderSession::new_with_trace(profile, REVISION_1.scene_surface(), scene, request);
    session.font_subset_session = Arc::clone(cache);
    let result = session.render(scene, request, None, svg_fonts::Payload::Subset);
    (result, session.meter.trace_bytes())
}

pub(super) fn font_subset_cache_stats(cache: &SharedFontSubsetSession) -> (usize, usize) {
    cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .stats()
}

pub(super) fn render_frame_with_trace(
    catalog: &'static RevisionCatalog,
    frame: FrameRef<'_>,
    request: &FrameRenderRequest,
    prepared: Option<&PreparedFrameRender>,
) -> (Result<RenderOutcome, RenderError>, Vec<u8>) {
    let profile = catalog.frame_profile();
    let request = request.as_render_request();
    let mut session =
        RenderSession::new_with_trace(profile, catalog.scene_surface(), frame.scene, request);
    session.target = RenderTarget::Frame(FrameTarget {
        source_index: frame.source_index,
        ordinal: frame.ordinal(),
        kind: frame.kind(),
    });
    if let Some(prepared) = prepared {
        session.font_subset_session = Arc::clone(&prepared.font_subset_session);
    }
    let result = session.render(frame.scene, request, prepared, svg_fonts::Payload::Subset);
    (result, session.meter.trace_bytes())
}

pub(super) fn prepared_font_subset_cache_stats(prepared: &PreparedFrameRender) -> (usize, usize) {
    prepared
        .font_subset_session
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .stats()
}

impl RenderSession {
    pub(super) fn new_with_trace(
        profile: &'static ReferenceProfileIdentity,
        scene_surface: &'static SceneSurfacePolicy,
        scene: &SceneDocument,
        request: &RenderRequest,
    ) -> Self {
        Self {
            profile,
            scene_surface,
            report: CompatibilityReport::empty(),
            meter: ResourceMeter::with_trace(profile, scene, request),
            staged_output: None,
            element_source_order_base: scene.source_owner_order("elements", 0),
            excalifont_selected: false,
            excalifont_range_selected: false,
            excalifont_glyphs: Vec::new(),
            excalifont_request_derived_bytes: 0,
            pending_png_animation_diagnostics: Vec::new(),
            pending_frame_normalization_diagnostics: Vec::new(),
            target: RenderTarget::Scene,
            diagnostic_attribution: DiagnosticAttribution::SceneGlobal,
            font_subset_session: Arc::new(Mutex::new(svg_fonts::Session::new())),
            svg_trees: SvgTreeCache::new(),
        }
    }

    pub(super) fn measure_and_emit_output(
        &mut self,
        request: &RenderRequest,
        error_code: ErrorCode,
        codec_emission_work: u64,
        mut encode: impl FnMut() -> Result<Vec<u8>, FormatEncodeError>,
    ) -> Result<(), RenderError> {
        let measured = encode().map_err(|error| self.format_failure(request, error_code, error))?;
        self.emit_premeasured_output(request, error_code, codec_emission_work, measured, encode)
    }
}
