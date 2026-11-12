//! Scene resolution: the phases of [`RenderSession::resolve_scene`] that turn
//! the admitted scene into the format-neutral [`ResolvedScene`] the encoder
//! consumes. Phase order — inputs, census, element resolution, assembly — is the
//! canonical charge order (ADR-0002); the journal-replay-vs-live and
//! scene-vs-frame variants end inside [`RenderSession::acquire_resolution_inputs`].

use crate::diagnostic_registry::LocalRecovery;
use crate::normalization::{
    normalize_scene_with_assets, NormalizationError, NormalizationOutcome, NormalizedElement,
    NormalizedElementKind, NormalizedFileRecord, NormalizedFreedraw, NormalizedLinear,
};
use crate::outcome::DiagnosticAttribution;
use crate::relationships::{self, RelationshipError, RelationshipOutcome};
use crate::resource_accounting::{ResourceContext, ResourceDelta, ResourcePhase};
use crate::scene::{self, SourceField};
use crate::scene_surface::SurfaceFallback;
use crate::{OutputFormat, RenderError, RenderRequest, SceneDocument};

use super::diagnostics::{
    diagnostic_replaces_source, diagnostic_uses_non_generic_recovery, known_element_fallback,
    known_element_replaced_diagnostic, relationship_cycle_diagnostic,
};
use super::frame_target::{
    selected_file_ids, EffectiveFrameRendering, FrameSelection, FrameViewport,
};
use super::geometry::{adjusted_roughness, generate_rough, GeneratedFreedraw, GeneratedGeometry};
use super::prepared::PreparedFrameRender;
use super::resolve::{
    coordinate_local_crossing, fallback_fits_local_limits, format_error_code,
    generation_local_crossing, logical_aligned_bytes, outline_local_crossing,
    relationship_asset_authorities, ResolveSceneError, ResolvedScene, SemanticWorkingRecord,
};
use super::{
    asset, element_owner_index, freedraw, image, rough, try_owned, DiagnosticOwnership,
    FormatEncodeError, RenderSession, RenderTarget,
};

fn surface_diagnostic_preflight_limit(request: &RenderRequest) -> usize {
    let limits = request.resource_limits();
    let diagnostic_limit = u64::from(limits.max_diagnostics()).saturating_add(1);
    let working_limit = limits
        .max_working_bytes()
        .saturating_div(512)
        .saturating_add(1);
    let work_limit = limits
        .max_total_work_units()
        .saturating_div(8)
        .saturating_add(1);
    usize::try_from(diagnostic_limit.min(working_limit).min(work_limit)).unwrap_or(usize::MAX)
}

/// Relationship resolution for one render: replayed from a prepared frame
/// journal, or computed live and owned here. Dereferences to the outcome so
/// later phases are indifferent to which variant produced it.
enum ResolvedRelationships<'p> {
    Replayed(&'p RelationshipOutcome),
    Live(RelationshipOutcome),
}

impl std::ops::Deref for ResolvedRelationships<'_> {
    type Target = RelationshipOutcome;

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Replayed(relationships) => relationships,
            Self::Live(relationships) => relationships,
        }
    }
}

/// Everything phase 1 settles: the frame-scoped normalization, the resolved
/// relationships, and the target scope. The journal-replay-vs-live and
/// scene-vs-frame variants never escape `acquire_resolution_inputs`.
struct ResolutionInputs<'p> {
    normalization: NormalizationOutcome,
    relationships: ResolvedRelationships<'p>,
    frame_viewport: Option<FrameViewport>,
    frame_participating_sources: Option<Vec<usize>>,
    selected_file_ids: Option<Vec<String>>,
}

/// The non-element scene state the assembly phase needs: file records, view
/// background, effective frame rendering, and the target viewport.
struct SceneChrome {
    cjk_subset_trigger: super::text::CjkSubsetTrigger,
    files: Vec<NormalizedFileRecord>,
    view_background_color: String,
    view_background_working_bytes: u64,
    frame_rendering: EffectiveFrameRendering,
    viewport: Option<FrameViewport>,
}

impl RenderSession {
    fn scope_duplicate_identity_effects(
        &mut self,
        scene: &SceneDocument,
        request: &RenderRequest,
        normalization: &NormalizationOutcome,
        relationships: &RelationshipOutcome,
    ) -> Result<(), RenderError> {
        let duplicate = |code: crate::DiagnosticCode| {
            code == crate::DiagnosticCode::NORMALIZATION_DUPLICATE_ELEMENT_ID
        };
        if !self
            .report
            .diagnostics
            .iter()
            .any(|diagnostic| duplicate(diagnostic.code()))
            && !self
                .pending_frame_normalization_diagnostics
                .iter()
                .any(|pending| duplicate(pending.diagnostic.code()))
        {
            return Ok(());
        }
        let frame_target = matches!(self.target, RenderTarget::Frame(_));
        // Each graph node's source `id` with its node position, sorted, so a
        // duplicate finds the first occurrence it duplicates by binary search
        // instead of comparing its id against every earlier element. The nodes
        // ascend by source index, so the first entry for an id is the earliest
        // occurrence: the one a scan from the front would find.
        let mut first_nodes = Vec::new();
        if first_nodes
            .try_reserve_exact(relationships.graph.nodes.len())
            .is_err()
        {
            return Err(self.memory_failure(
                request,
                "compatibility_report",
                "duplicate_identity_effects",
            ));
        }
        for (position, node) in relationships.graph.nodes.iter().enumerate() {
            if let Some(crate::scene::JsonValue::Object(members)) =
                scene.element_value(node.source_index)
            {
                if let Some(crate::scene::JsonValue::String(id)) =
                    scene::member_value(members, "id")
                {
                    first_nodes.push((&**id, position));
                }
            }
        }
        first_nodes.sort_unstable();
        let visual_for = |pointer: &str| {
            let Some(index) = element_owner_index(pointer).and_then(|i| usize::try_from(i).ok())
            else {
                return false;
            };
            let Some(crate::scene::JsonValue::Object(members)) = scene.element_value(index) else {
                return false;
            };
            let Some(crate::scene::JsonValue::String(original)) =
                scene::member_value(members, "id")
            else {
                return false;
            };
            let at = first_nodes.partition_point(|(id, _)| *id < &**original);
            let first = first_nodes
                .get(at)
                .filter(|(id, _)| *id == &**original)
                .and_then(|(_, position)| relationships.graph.nodes.get(*position))
                .filter(|node| node.source_index < index);
            let Some(first) = first else {
                return false;
            };
            let graph = &relationships.graph;
            let element_at = |index| {
                normalization
                    .elements
                    .iter()
                    .find(|e| e.source.source_index() == index)
            };
            let nonempty_text = |element: Option<&NormalizedElement>| {
                element.is_some_and(|e| matches!(&e.kind, NormalizedElementKind::Text(text) if !text.text.is_empty()))
            };
            graph.edges.iter().any(|edge| {
                let relationships::RelationshipTarget::Element(target) = edge.target else { return false; };
                let Some(target_node) = graph.nodes.get(target) else { return false; };
                if target_node.source_index != first.source_index { return false; }
                let Some(source_node) = graph.nodes.get(edge.source) else { return false; };
                let source_index = source_node.source_index;
                let source = element_at(source_index);
                let fallback = normalization.fallbacks.iter().any(|f| f.source_index == source_index);
                if source.is_none() && !fallback { return false; }
                let target = element_at(first.source_index);
                match edge.kind {
                    relationships::RelationshipKind::Frame | relationships::RelationshipKind::Clip => {
                        let paints = source.is_none_or(|e| match &e.kind {
                            NormalizedElementKind::Text(text) => !text.text.is_empty() &&
                                !graph.edges.iter().any(|mask| mask.source == edge.source && mask.kind == relationships::RelationshipKind::Mask &&
                                    matches!(mask.target, relationships::RelationshipTarget::Element(t) if graph.nodes.get(t).is_some_and(|container| !super::bound_text::has_reciprocal_bound_text(container.source_index, source_index, graph)))),
                            _ => true,
                        });
                        target.is_some_and(|e| matches!(e.kind, NormalizedElementKind::Frame(_) | NormalizedElementKind::MagicFrame(_))) &&
                            (frame_target || (normalization.frame_rendering.enabled && normalization.frame_rendering.clip && request.frame_options().enabled() && request.frame_options().clip()) || (paints && target.is_some_and(|e| e.opacity != 100.0)))
                    }
                    relationships::RelationshipKind::Binding | relationships::RelationshipKind::Mask => {
                        if frame_target && target.is_some() { return true; }
                        if edge.json_pointer.ends_with("/containerId") {
                            // Choosing a nonreciprocal container suppresses text,
                            // which is itself a visual/semantic consequence.
                            nonempty_text(source) && target.is_some()
                        } else if edge.json_pointer.contains("/boundElements/") || edge.json_pointer.contains("/boundElementIds/") {
                            graph.edges.iter().any(|candidate| {
                                candidate.kind == relationships::RelationshipKind::Mask &&
                                    graph.nodes.get(candidate.source).is_some_and(|node| (node.source_index == first.source_index || node.source_index == index) && nonempty_text(element_at(node.source_index))) &&
                                    matches!(candidate.target, relationships::RelationshipTarget::Element(t) if graph.nodes.get(t).is_some_and(|node| node.source_index == source_index))
                            })
                        } else { false }
                    }
                    _ => false,
                }
            })
        };
        for diagnostic in &mut self.report.diagnostics {
            if diagnostic.code() == crate::DiagnosticCode::NORMALIZATION_DUPLICATE_ELEMENT_ID
                && diagnostic
                    .set_identity_effects(visual_for(diagnostic.json_pointer()))
                    .is_err()
            {
                return Err(self.memory_failure(
                    request,
                    "compatibility_report",
                    "duplicate_identity_effects",
                ));
            }
        }
        for pending in &mut self.pending_frame_normalization_diagnostics {
            if pending.diagnostic.code()
                == crate::DiagnosticCode::NORMALIZATION_DUPLICATE_ELEMENT_ID
                && pending
                    .diagnostic
                    .set_identity_effects(visual_for(pending.diagnostic.json_pointer()))
                    .is_err()
            {
                return Err(self.memory_failure(
                    request,
                    "compatibility_report",
                    "duplicate_identity_effects",
                ));
            }
        }
        Ok(())
    }
    /// Effects describe channels the repaired source relationship can affect in
    /// this target, rather than the number or kind of its graph projections.
    fn cycle_effects(
        &self,
        cycle: &relationships::RemovedCycle,
        inputs: &ResolutionInputs<'_>,
        request: &RenderRequest,
    ) -> (bool, bool) {
        let elements = &inputs.normalization.elements;
        let source = elements
            .iter()
            .find(|element| element.source.source_index() == cycle.source_index);
        let target = elements
            .iter()
            .find(|element| element.source.source_index() == cycle.target_source_index);
        // Frame scoping has already removed nonselected normalized elements.
        // A rejected binding can have prevented selection of its target, so
        // retain its pre-scope participation from the graph's normalized bounds.
        let target_was_normalized = inputs.relationships.graph.nodes.iter().any(|node| {
            node.source_index == cycle.target_source_index && node.hit_bounds.is_some()
        });
        let nonempty_text = |element: Option<&NormalizedElement>| {
            element.is_some_and(|element| matches!(&element.kind, NormalizedElementKind::Text(text) if !text.text.is_empty()))
        };
        let frame_target = matches!(self.target, RenderTarget::Frame(_));
        let frame_rendering = inputs.normalization.frame_rendering;
        let scene_clip = frame_rendering.enabled
            && frame_rendering.clip
            && request.frame_options().enabled()
            && request.frame_options().clip();
        let visual = match cycle.channel {
            relationships::CycleChannel::FrameMembership => {
                // Frame opacity is inherited even when outline/clipping
                // controls are disabled (resolve_frame_opacity).
                let frame_opacity = target.is_some_and(|element| {
                    matches!(
                        element.kind,
                        NormalizedElementKind::Frame(_) | NormalizedElementKind::MagicFrame(_)
                    ) && element.opacity != 100.0
                });
                // Membership filters frame exports even when PNG omits clips;
                // scene bounds consume it whenever scene clipping is enabled.
                (source.is_some()
                    || inputs
                        .normalization
                        .fallbacks
                        .iter()
                        .any(|fallback| fallback.source_index == cycle.source_index))
                    && (frame_target || scene_clip || frame_opacity)
            }
            relationships::CycleChannel::TextContainer => {
                nonempty_text(source) || (frame_target && source.is_some() && target_was_normalized)
            }
            relationships::CycleChannel::BoundElement => {
                // A retained reciprocal Mask has the same dependency direction
                // as this reverse binding, so it cannot coexist with the path
                // that rejects the binding. Without that Mask, text is unbound;
                // only frame selection can consume the rejected reference.
                frame_target && source.is_some() && target_was_normalized
            }
            relationships::CycleChannel::EndpointBinding => {
                frame_target && source.is_some() && target_was_normalized
            }
        };
        (visual, visual && request.format() != OutputFormat::Png)
    }

    /// Normalizes, relates, frame-scopes, and resolves the scene into the
    /// format-independent plan the encoder consumes. Phase order — inputs,
    /// census, element resolution, assembly — is the canonical charge order.
    pub(super) fn resolve_scene(
        &mut self,
        scene: &SceneDocument,
        request: &RenderRequest,
        prepared: Option<&PreparedFrameRender>,
        semantic_working: &mut Vec<SemanticWorkingRecord>,
    ) -> Result<ResolvedScene, RenderError> {
        let inputs = self.acquire_resolution_inputs(scene, request, prepared)?;
        let mut images = image::ImageResolver::new(
            relationship_asset_authorities(&inputs.relationships.graph).map_err(|_| {
                self.memory_failure(request, "relationship_resolution", "asset_authorities")
            })?,
        );
        self.account_semantic_census(scene, request, &inputs, semantic_working)?;
        let frame_rendering = inputs.normalization.frame_rendering;
        let view_background_color = inputs.normalization.view_background_color;
        let view_background_working_bytes = inputs.normalization.view_background_working_bytes;
        let effective_frame_rendering = match self.target {
            RenderTarget::Scene => EffectiveFrameRendering {
                enabled: frame_rendering.enabled && request.frame_options().enabled(),
                clip: frame_rendering.clip && request.frame_options().clip(),
                name: frame_rendering.name && request.frame_options().show_name(),
                outline: frame_rendering.outline && request.frame_options().outline(),
                rectangular_clip: false,
                emit_frame_elements: true,
            },
            RenderTarget::Frame(_) => EffectiveFrameRendering {
                enabled: true,
                clip: request.format() != OutputFormat::Png,
                name: false,
                outline: false,
                rectangular_clip: true,
                emit_frame_elements: false,
            },
        };
        let chrome = SceneChrome {
            cjk_subset_trigger: super::resolve::text::cjk_subset_trigger(
                inputs.normalization.elements.iter(),
                &inputs.relationships.graph,
            ),
            files: inputs.normalization.files,
            view_background_color,
            view_background_working_bytes,
            frame_rendering: effective_frame_rendering,
            viewport: inputs.frame_viewport,
        };
        let mut fallbacks = inputs.normalization.fallbacks;
        // Preserve the invalid frame's clipping boundary by omitting its
        // dependants too; retaining children would expose unclipped content.
        let mut rejected_frames = Vec::new();
        rejected_frames
            .try_reserve(inputs.normalization.elements.len())
            .map_err(|_| {
                self.memory_failure(request, "semantic_geometry", "rejected_frame_coordinates")
            })?;
        for source in &inputs.normalization.elements {
            if matches!(
                source.kind,
                NormalizedElementKind::Frame(_) | NormalizedElementKind::MagicFrame(_)
            ) && coordinate_local_crossing(source, request).is_some()
            {
                rejected_frames.push(source.source.source_index());
            }
        }
        let clips_active = effective_frame_rendering.enabled && effective_frame_rendering.clip;
        if clips_active {
            loop {
                let before = rejected_frames.len();
                for source in &inputs.normalization.elements {
                    if matches!(
                        source.kind,
                        NormalizedElementKind::Frame(_) | NormalizedElementKind::MagicFrame(_)
                    ) && super::frame_target::relationship_frame_target(
                        source.source.source_index(),
                        &inputs.relationships.graph,
                    )
                    .is_some_and(|index| rejected_frames.contains(&index))
                        && !rejected_frames.contains(&source.source.source_index())
                    {
                        rejected_frames.push(source.source.source_index());
                    }
                }
                if rejected_frames.len() == before {
                    break;
                }
            }
        }
        let mut omitted_by_frame = Vec::new();
        omitted_by_frame
            .try_reserve(inputs.normalization.elements.len())
            .map_err(|_| {
                self.memory_failure(request, "semantic_geometry", "rejected_frame_descendants")
            })?;
        for source in &inputs.normalization.elements {
            if clips_active
                && super::frame_target::relationship_frame_target(
                    source.source.source_index(),
                    &inputs.relationships.graph,
                )
                .is_some_and(|index| rejected_frames.contains(&index))
            {
                omitted_by_frame.push(source.source.source_index());
            }
        }
        let keep_fallback = |fallback: &SurfaceFallback| {
            !clips_active
                || !super::frame_target::relationship_frame_target(
                    fallback.source_index,
                    &inputs.relationships.graph,
                )
                .is_some_and(|index| rejected_frames.contains(&index))
        };
        fallbacks
            .iter()
            .try_for_each(|fallback| {
                if !keep_fallback(fallback) {
                    for diagnostic in &mut self.report.diagnostics {
                        if diagnostic.element_id() == fallback.element_id.as_deref()
                            && diagnostic.json_pointer() == fallback.json_pointer
                        {
                            diagnostic.omit_surface_fallback(fallback.link.is_some())?;
                        }
                    }
                }
                Ok(())
            })
            .map_err(|_: std::collections::TryReserveError| {
                self.memory_failure(request, "compatibility_report", "frame_fallback_omission")
            })?;
        fallbacks.retain(keep_fallback);
        let mut outlines = Vec::new();
        outlines
            .try_reserve_exact(inputs.normalization.elements.len())
            .map_err(|_| self.memory_failure(request, "normalization", "outline_list"))?;
        for mut source in inputs.normalization.elements {
            debug_assert!(!source.index.is_empty());
            let frame_omission = omitted_by_frame.contains(&source.source.source_index());
            let non_outline = matches!(
                source.kind,
                NormalizedElementKind::Image(_)
                    | NormalizedElementKind::Text(_)
                    | NormalizedElementKind::Frame(_)
                    | NormalizedElementKind::MagicFrame(_)
            );
            let coordinate_crossing = non_outline
                .then(|| coordinate_local_crossing(&source, request))
                .flatten();
            if frame_omission || coordinate_crossing.is_some() {
                if !frame_omission {
                    let crossing = coordinate_crossing.unwrap();
                    let position = crossing
                        .pointer_field
                        .map_or(u64::MAX, |field| source.source.coordinate_position(field));
                    let semantic = match &source.kind {
                        NormalizedElementKind::Text(text) => super::resolve::live_text_occurrence(
                            text,
                            source.source.source_index(),
                            &inputs.relationships.graph,
                        )
                        .is_some(),
                        NormalizedElementKind::Image(image) => {
                            image.file_id.as_deref().is_some_and(|id| {
                                asset::rejected_image_may_have_semantics(
                                    scene,
                                    request.supplemental_assets(),
                                    id,
                                    source.link.is_some(),
                                )
                            })
                        }
                        _ => true,
                    };
                    let diagnostic = crossing
                        .diagnostic_with_recovery(
                            request.format(),
                            Some(source.source.id()),
                            source.source.pointer(),
                            LocalRecovery::Omitted,
                            semantic,
                        )
                        .map_err(|_| {
                            self.memory_failure(
                                request,
                                "compatibility_report",
                                "coordinate_recovery",
                            )
                        })?;
                    self.retain_diagnostic_with_owner(
                        request,
                        ResourcePhase::SharedSemanticResolution,
                        "coordinate_local_resource_recovery",
                        diagnostic,
                        DiagnosticOwnership {
                            local_owner: Some(source.source.id()),
                            source_order: Some(source.source_order),
                            source_position: Some(position),
                        },
                    )?;
                }
                self.charge_local_omission(
                    request,
                    Some(source.source.id()),
                    source.source.pointer(),
                    "coordinate_local_omission",
                )?;
                semantic_working.push(SemanticWorkingRecord {
                    source_order: source.source_order,
                    event: "coordinate_omitted_source",
                    element_id: Some(try_owned(source.source.id()).map_err(|_| {
                        self.memory_failure(request, "semantic_geometry", "coordinate_owner")
                    })?),
                    local_owner: Some(try_owned(source.source.id()).map_err(|_| {
                        self.memory_failure(request, "semantic_geometry", "coordinate_owner")
                    })?),
                    json_pointer: try_owned(source.source.pointer()).map_err(|_| {
                        self.memory_failure(request, "semantic_geometry", "coordinate_pointer")
                    })?,
                    bytes: source.normalized_working_bytes,
                });
                continue;
            }
            if matches!(source.kind, NormalizedElementKind::Image(_)) {
                match images.resolve_element(self, scene, request, source, semantic_working)? {
                    image::ImageDisposition::Outline(element) => {
                        outlines.push((element, None));
                        continue;
                    }
                    image::ImageDisposition::Generic(element) => source = element,
                }
            }
            if let NormalizedElementKind::Embeddable { iframe, .. } = &source.kind {
                let both_transparent = source.stroke_color == "transparent"
                    && source.background_color == "transparent";
                if both_transparent {
                    source.background_color = try_owned("#d3d3d3").map_err(|_| {
                        self.memory_failure(request, "normalization", "embeddable_color")
                    })?;
                    source.fill_style = try_owned("solid").map_err(|_| {
                        self.memory_failure(request, "normalization", "embeddable_fill")
                    })?;
                    source.roughness = 0.0;
                } else if *iframe {
                    if source.background_color == "transparent" {
                        source.background_color = try_owned("#f4f4f6").map_err(|_| {
                            self.memory_failure(request, "normalization", "iframe_background")
                        })?;
                    }
                    if source.stroke_color == "transparent" {
                        source.stroke_color = try_owned("#000000").map_err(|_| {
                            self.memory_failure(request, "normalization", "iframe_stroke")
                        })?;
                    }
                }
            }
            let is_outline = matches!(
                &source.kind,
                NormalizedElementKind::Rectangle
                    | NormalizedElementKind::Diamond
                    | NormalizedElementKind::Ellipse
                    | NormalizedElementKind::Embeddable { .. }
                    | NormalizedElementKind::Linear(NormalizedLinear {
                        geometry_available: true,
                        ..
                    })
                    | NormalizedElementKind::Freedraw(NormalizedFreedraw {
                        geometry_available: true,
                        ..
                    })
            );
            if matches!(
                source.kind,
                NormalizedElementKind::Frame(_) | NormalizedElementKind::MagicFrame(_)
            ) {
                semantic_working.push(SemanticWorkingRecord {
                    source_order: source.source_order,
                    event: "resolved_frame",
                    element_id: Some(try_owned(source.source.id()).map_err(|_| {
                        self.memory_failure(request, "normalization", "working_record_identity")
                    })?),
                    local_owner: Some(try_owned(source.source.id()).map_err(|_| {
                        self.memory_failure(request, "normalization", "working_record_owner")
                    })?),
                    json_pointer: try_owned(source.source.pointer()).map_err(|_| {
                        self.memory_failure(request, "normalization", "working_record_pointer")
                    })?,
                    bytes: source.normalized_working_bytes,
                });
                outlines.push((source, None));
                continue;
            }
            if matches!(&source.kind, NormalizedElementKind::Text(_)) {
                if self.preflight_official_text(
                    &source,
                    &inputs.relationships.graph,
                    request,
                    semantic_working,
                    &mut fallbacks,
                )? {
                    outlines.push((source, None));
                }
                continue;
            }
            if !is_outline || !source.direct_outline_compatible {
                let uses_non_generic_recovery =
                    self.report.diagnostics.iter().any(|diagnostic| {
                        diagnostic_uses_non_generic_recovery(&source, diagnostic)
                    });
                if !self
                    .report
                    .diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic_replaces_source(&source, diagnostic))
                {
                    let diagnostic = known_element_replaced_diagnostic(
                        request.format(),
                        source.source.id(),
                        source.source.pointer(),
                        source.width > 0.0 && source.height > 0.0,
                    )
                    .map_err(|_| {
                        self.memory_failure(
                            request,
                            "compatibility_report",
                            "known_element_replacement",
                        )
                    })?;
                    self.retain_diagnostic_with_owner(
                        request,
                        ResourcePhase::SharedSemanticResolution,
                        "known_element_replacement",
                        diagnostic,
                        DiagnosticOwnership {
                            local_owner: Some(source.source.id()),
                            source_order: Some(source.source_order),
                            source_position: Some(0),
                        },
                    )?;
                }
                semantic_working.push(SemanticWorkingRecord {
                    source_order: source.source_order,
                    event: "resolved_known_element_fallback",
                    element_id: Some(try_owned(source.source.id()).map_err(|_| {
                        self.memory_failure(request, "normalization", "working_record_identity")
                    })?),
                    local_owner: Some(try_owned(source.source.id()).map_err(|_| {
                        self.memory_failure(request, "normalization", "working_record_owner")
                    })?),
                    json_pointer: try_owned(source.source.pointer()).map_err(|_| {
                        self.memory_failure(request, "normalization", "working_record_pointer")
                    })?,
                    bytes: source.normalized_working_bytes,
                });
                if source.width > 0.0 && source.height > 0.0 {
                    fallbacks.try_reserve(1).map_err(|_| {
                        self.memory_failure(request, "semantic_geometry", "fallback_list")
                    })?;
                    let mut fallback = known_element_fallback(&source).map_err(|_| {
                        self.memory_failure(request, "semantic_geometry", "known_element_fallback")
                    })?;
                    fallback.paint = !uses_non_generic_recovery;
                    fallbacks.push(fallback);
                }
                continue;
            }
            let has_outline = match &source.kind {
                NormalizedElementKind::Linear(linear) => linear
                    .points
                    .windows(2)
                    .any(|points| points[0] != points[1]),
                NormalizedElementKind::Freedraw(freedraw) => {
                    !freedraw.points.is_empty() || !freedraw.simulate_pressure
                }
                NormalizedElementKind::Embeddable { .. } => true,
                _ => source.width != 0.0 && source.height != 0.0,
            };
            let base_element_work_units = source
                .normalization_work_units
                .saturating_add(8)
                .saturating_add(source.repair_work_units);
            // Coordinate and already-spent work admission must precede any Rough allocation.
            let mut local_crossing =
                outline_local_crossing(&source, request, 0, base_element_work_units);
            let mut rejected_geometry_segments = 0;
            let mut generation_work = 0;
            let geometry = if has_outline && local_crossing.is_none() {
                let roughness = adjusted_roughness(&source);
                let mut budget = rough::GenerationBudget::new(
                    u64::from(
                        request
                            .resource_limits()
                            .max_geometry_segments_per_element(),
                    ),
                    request.resource_limits().max_element_work_units(),
                    base_element_work_units,
                )
                .with_coordinate_limit(request.resource_limits().max_coordinate_magnitude());
                let generated = match &source.kind {
                    NormalizedElementKind::Freedraw(freedraw) => {
                        freedraw::generate(freedraw, source.stroke_width, &mut budget).and_then(
                            |path| {
                                let background_fill = if source.background_color == "transparent" {
                                    None
                                } else {
                                    freedraw::background_fill(
                                        &freedraw.points,
                                        source.seed,
                                        roughness,
                                        source.roughness < 2.0,
                                        source.stroke_style != "solid",
                                        rough::FillStyle::from_normalized(&source.fill_style),
                                        source.stroke_width,
                                        &mut budget,
                                    )?
                                };
                                Ok(GeneratedGeometry::Freedraw(GeneratedFreedraw {
                                    path,
                                    background_fill,
                                }))
                            },
                        )
                    }
                    _ => generate_rough(&source, roughness, &mut budget)
                        .map(GeneratedGeometry::Rough),
                };
                generation_work = budget.spent_work();
                match generated {
                    Ok(geometry) => Some(geometry),
                    Err(rough::GenerationError::Coordinate {
                        value,
                        geometry_segments,
                    }) => {
                        rejected_geometry_segments = geometry_segments;
                        local_crossing = Some(super::coordinates::crossing(value, request));
                        None
                    }
                    Err(rough::GenerationError::Memory) => {
                        return Err(self.memory_failure(
                            request,
                            "semantic_geometry",
                            "rough_outline",
                        ));
                    }
                    Err(rough::GenerationError::Limit {
                        kind,
                        attempted,
                        geometry_segments,
                    }) => {
                        rejected_geometry_segments = geometry_segments;
                        local_crossing = Some(generation_local_crossing(kind, attempted, request));
                        None
                    }
                }
            } else {
                None
            };
            // Generation guard work was performed whether or not the outline
            // is admitted, so it commits to the aggregate either way: the
            // element ceiling bounds one outline, total work bounds them all.
            if generation_work != 0 {
                self.charge_work(
                    request,
                    ResourceContext::new(
                        ResourcePhase::SharedSemanticResolution,
                        "rough_generation_guard",
                        source.source.pointer(),
                    )
                    .with_element_id(Some(source.source.id()))
                    .with_local_owner(Some(source.source.id())),
                    generation_work.saturating_mul(8),
                )?;
            }
            let geometry_segments = geometry
                .as_ref()
                .map_or(rejected_geometry_segments, |geometry| {
                    u64::try_from(geometry.operation_count()).unwrap_or(u64::MAX)
                });
            let geometry_work_units = geometry_segments * 32;
            let geometry_working_bytes = if geometry.is_some() {
                128 + geometry_segments * 64
            } else {
                0
            };
            let geometry_delta = ResourceDelta {
                geometry_segments,
                working_bytes: geometry_working_bytes,
                total_work_units: geometry_work_units,
                ..ResourceDelta::default()
            };
            let geometry_event = match source.kind {
                NormalizedElementKind::Rectangle => "rough_rectangle_geometry",
                NormalizedElementKind::Diamond => "rough_diamond_geometry",
                NormalizedElementKind::Ellipse => "rough_ellipse_geometry",
                NormalizedElementKind::Embeddable { .. } => "rough_embeddable_geometry",
                NormalizedElementKind::Linear(_) => "rough_linear_geometry",
                NormalizedElementKind::Freedraw(_) => "freedraw_geometry",
                NormalizedElementKind::Text(_)
                | NormalizedElementKind::Image(_)
                | NormalizedElementKind::Frame(_)
                | NormalizedElementKind::MagicFrame(_) => {
                    unreachable!("only direct outlines reach geometry accounting")
                }
            };
            let geometry_context = ResourceContext::new(
                ResourcePhase::SharedSemanticResolution,
                geometry_event,
                source.source.pointer(),
            )
            .with_element_id(Some(source.source.id()))
            .with_local_owner(Some(source.source.id()));
            if self.meter.would_cross_fatal(geometry_delta) {
                return match self.transact(request, geometry_context, geometry_delta) {
                    Err(error) => Err(error),
                    Ok(()) => Err(self.internal_failure(
                        request.format(),
                        "fatal resource preflight committed unexpectedly",
                        "/elements",
                    )),
                };
            }
            let element_work_units = base_element_work_units.saturating_add(geometry_work_units);
            if local_crossing.is_none() {
                local_crossing =
                    outline_local_crossing(&source, request, geometry_segments, element_work_units);
            }
            if local_crossing.is_none() {
                local_crossing = geometry.as_ref().and_then(|geometry| {
                    super::coordinates::generated(&source, geometry, request)
                        .err()
                        .map(|value| super::coordinates::crossing(value, request))
                });
            }
            let include_outline = has_outline && local_crossing.is_none();
            let rejected = local_crossing.as_ref().is_some_and(|crossing| {
                crossing.limit == crate::diagnostic_registry::LocalLimit::CoordinateMagnitude
            });
            let semantic_working_bytes = if let Some(crossing) = local_crossing {
                self.commit_local_rejection_observation(
                    request,
                    geometry_context,
                    ResourceDelta {
                        geometry_segments,
                        ..ResourceDelta::default()
                    },
                )?;
                let source_position = crossing
                    .pointer_field
                    .map_or(u64::MAX, |field| source.source.coordinate_position(field));
                let replacement = if source.width > 0.0 && source.height > 0.0 {
                    let fallback = known_element_fallback(&source).map_err(|_| {
                        self.memory_failure(request, "semantic_geometry", "known_element_fallback")
                    })?;
                    fallback_fits_local_limits(&fallback, request.resource_limits())
                        .then_some(fallback)
                } else {
                    None
                };
                let can_replace = replacement.is_some();
                let diagnostic = crossing
                    .diagnostic_with_recovery(
                        request.format(),
                        Some(source.source.id()),
                        source.source.pointer(),
                        LocalRecovery::placeholder_if(can_replace),
                        has_outline,
                    )
                    .map_err(|_| {
                        self.memory_failure(request, "compatibility_report", "local_recovery")
                    })?;
                self.retain_diagnostic_with_owner(
                    request,
                    ResourcePhase::SharedSemanticResolution,
                    "outline_local_resource_recovery",
                    diagnostic,
                    DiagnosticOwnership {
                        local_owner: Some(source.source.id()),
                        source_order: Some(source.source_order),
                        source_position: Some(source_position),
                    },
                )?;
                if let Some(fallback) = replacement {
                    fallbacks.try_reserve(1).map_err(|_| {
                        self.memory_failure(request, "semantic_geometry", "fallback_list")
                    })?;
                    fallbacks.push(fallback);
                } else {
                    self.charge_local_omission(
                        request,
                        Some(source.source.id()),
                        source.source.pointer(),
                        "outline_local_omission",
                    )?;
                }
                source.normalized_working_bytes
            } else if has_outline {
                self.transact(request, geometry_context, geometry_delta)?;
                source
                    .normalized_working_bytes
                    .saturating_add(geometry_working_bytes)
            } else {
                source.normalized_working_bytes
            };
            semantic_working.push(SemanticWorkingRecord {
                source_order: source.source_order,
                event: "resolved_outline",
                element_id: Some(try_owned(source.source.id()).map_err(|_| {
                    self.memory_failure(request, "normalization", "working_record_identity")
                })?),
                local_owner: Some(try_owned(source.source.id()).map_err(|_| {
                    self.memory_failure(request, "normalization", "working_record_owner")
                })?),
                json_pointer: try_owned(source.source.pointer()).map_err(|_| {
                    self.memory_failure(request, "normalization", "working_record_pointer")
                })?,
                bytes: semantic_working_bytes,
            });
            if !rejected {
                outlines.push((source, include_outline.then_some(geometry).flatten()));
            }
        }
        self.assemble_resolved_scene(
            scene,
            request,
            outlines,
            images,
            fallbacks,
            chrome,
            &inputs.relationships,
            semantic_working,
        )
    }

    fn check_frame_coordinate_viewport(
        &mut self,
        source: &NormalizedElement,
        request: &RenderRequest,
    ) -> Result<(), RenderError> {
        let Some(crossing) = coordinate_local_crossing(source, request) else {
            return Ok(());
        };
        self.flush_frame_normalization_diagnostics(request, &[source.source.source_index()], &[])?;
        self.scope_frame_diagnostics(&[source.source.source_index()]);
        Err(RenderError {
            category: crate::RenderErrorCategory::ResourceLimit,
            code: crate::ErrorCode::RESOURCE_COORDINATE_RANGE_EXCEEDED,
            message: Box::from("selected frame viewport exceeds the coordinate ceiling"),
            format: request.format(),
            element_id: Some(try_owned(source.source.id()).map_err(|_| {
                self.memory_failure(request, "frame_selection", "coordinate_owner")
            })?),
            json_pointer: crossing
                .pointer_field
                .map_or_else(
                    || try_owned(source.source.pointer()),
                    |field| crate::scene::pointer_child(source.source.pointer(), field),
                )
                .map_err(|_| self.memory_failure(request, "frame_selection", "coordinate_pointer"))?
                .into_boxed_str(),
            details: Some(Box::new(crate::DiagnosticDetails::ResourceLimit {
                field: "max_coordinate_magnitude",
                configured: crossing.configured,
                value_kind: crate::ValueKind::Coordinate,
                measurement: crossing.attempted,
                recovery: None,
            })),
            report: self.take_report(),
        })
    }

    /// Phase 1: obtains the normalized scene and its relationships — replayed
    /// from the prepared journal or computed live — and applies frame scoping.
    /// Both variation axes (journal-replay vs live, scene vs frame target)
    /// end here; later phases see only [`ResolutionInputs`].
    fn acquire_resolution_inputs<'p>(
        &mut self,
        scene: &SceneDocument,
        request: &RenderRequest,
        prepared: Option<&'p PreparedFrameRender>,
    ) -> Result<ResolutionInputs<'p>, RenderError> {
        let (mut normalization, relationships) = if let Some(prepared) = prepared {
            // The journal is replayed event by event rather than collapsed into
            // one aggregate charge. Lump-sum charging was measured against
            // `frame_batch_property`: artifact bytes and reports survive it,
            // but the canonical resource trace does not, and the revision-1
            // contract requires the trace to be exact across the cold,
            // warm, request-cache, and frame-batch-cache paths alike.
            for event in &prepared.normalization_events {
                event.replay(self, request)?;
            }
            for event in &prepared.relationship_events {
                event.replay(self, request)?;
            }
            let target = match self.target {
                RenderTarget::Frame(target) => target,
                RenderTarget::Scene => unreachable!("prepared rendering is frame-scoped"),
            };
            if let Some(source) = prepared
                .normalization
                .elements
                .iter()
                .find(|source| source.source.source_index() == target.source_index)
            {
                self.check_frame_coordinate_viewport(source, request)?;
            }
            let normalization = prepared
                .normalization_for_target(target)
                .map_err(|_| self.memory_failure(request, "frame_selection", "prepared_target"))?;
            (
                normalization,
                ResolvedRelationships::Replayed(&prepared.relationships),
            )
        } else {
            let normalization = normalize_scene_with_assets(
                scene,
                self.profile.compatibility_revision(),
                self.scene_surface,
                request.format(),
                request.supplemental_assets(),
                matches!(self.target, RenderTarget::Scene)
                    && request.frame_options().enabled()
                    && request.frame_options().show_name(),
                if matches!(self.target, RenderTarget::Frame(_)) {
                    usize::MAX
                } else {
                    surface_diagnostic_preflight_limit(request)
                },
                |event| self.observe_normalization_event(request, event),
            );
            let normalization = match normalization {
                Ok(normalization) => normalization,
                Err(NormalizationError::Observer(error)) => return Err(error),
                Err(NormalizationError::MemoryUnavailable { source_index }) => {
                    let json_pointer = source_index
                        .and_then(|index| scene::pointer_index("/elements", index).ok())
                        .unwrap_or_default();
                    return Err(self.memory_failure_at(
                        request,
                        "normalization",
                        "record",
                        None,
                        json_pointer,
                    ));
                }
            }
            .ok_or_else(|| {
                self.internal_failure(
                    request.format(),
                    "nonempty scene is outside the implemented outline slice",
                    "/elements",
                )
            })?;
            let relationships = relationships::resolve(
                scene,
                &normalization.elements,
                request.supplemental_assets(),
                |event| self.observe_relationship_event(request, event),
            );
            let relationships = match relationships {
                Ok(relationships) => relationships,
                Err(RelationshipError::Observer(error)) => {
                    return Err(error);
                }
                Err(RelationshipError::MemoryUnavailable) => {
                    return Err(self.memory_failure(
                        request,
                        "relationship_resolution",
                        "relationship_graph",
                    ));
                }
            };
            (normalization, ResolvedRelationships::Live(relationships))
        };
        let (frame_viewport, frame_participating_sources, selected_file_ids) = match self.target {
            RenderTarget::Scene => {
                self.scope_duplicate_identity_effects(
                    scene,
                    request,
                    &normalization,
                    &relationships,
                )?;
                (None, None, None)
            }
            RenderTarget::Frame(target) => {
                if let Some(source) = normalization
                    .elements
                    .iter()
                    .find(|source| source.source.source_index() == target.source_index)
                {
                    self.check_frame_coordinate_viewport(source, request)?;
                }
                let selection =
                    FrameSelection::compute(&normalization.elements, &relationships.graph, target)
                        .map_err(|_| {
                            self.memory_failure(
                                request,
                                "frame_selection",
                                "participating_elements",
                            )
                        })?;
                let Some(selection) = selection else {
                    self.flush_frame_normalization_diagnostics(
                        request,
                        &[target.source_index],
                        &[],
                    )?;
                    self.scope_frame_diagnostics(&[target.source_index]);
                    self.diagnostic_attribution = DiagnosticAttribution::Target;
                    return Err(self.internal_failure(
                        request.format(),
                        "discovered frame target was not normalized",
                        "/elements",
                    ));
                };
                normalization
                    .elements
                    .retain(|element| selection.retains_element(element));
                normalization
                    .fallbacks
                    .retain(|fallback| selection.retains_fallback(fallback));
                let mut diagnostic_sources = selection.participating.clone();
                diagnostic_sources.extend(
                    normalization
                        .fallbacks
                        .iter()
                        .map(|fallback| fallback.source_index),
                );
                diagnostic_sources.sort_unstable();
                diagnostic_sources.dedup();
                let file_ids = selected_file_ids(&normalization.elements);
                normalization
                    .files
                    .retain(|record| file_ids.binary_search(&record.key).is_ok());
                for index in 0..self.pending_frame_normalization_diagnostics.len() {
                    self.pending_frame_normalization_diagnostics[index].diagnostic.scope_forbidden_resource_recovery(|file_id| {
                        normalization.elements.iter().any(|source| matches!(&source.kind,NormalizedElementKind::Image(image)
                            if image.file_id.as_deref()==Some(file_id) && source.width>0.0 && source.height>0.0))
                    }).map_err(|_| self.memory_failure(request,"frame_selection","forbidden_resource_recovery"))?;
                    self.pending_frame_normalization_diagnostics[index]
                        .diagnostic
                        .scope_malformed_image_recovery(|file_id| {
                            normalization
                                .elements
                                .iter()
                                .filter_map(|element| match &element.kind {
                                    NormalizedElementKind::Image(image)
                                        if image.file_id.as_deref() == Some(file_id) =>
                                    {
                                        Some((element.source.source_index(), image.status))
                                    }
                                    _ => None,
                                })
                                .min_by_key(|(index, _)| *index)
                                .is_some_and(|(_, status)| {
                                    status == crate::normalization::NormalizedImageStatus::Error
                                })
                        })
                        .map_err(|_| {
                            self.memory_failure(
                                request,
                                "frame_selection",
                                "malformed_image_recovery",
                            )
                        })?;
                }
                self.scope_duplicate_identity_effects(
                    scene,
                    request,
                    &normalization,
                    &relationships,
                )?;
                self.flush_frame_normalization_diagnostics(
                    request,
                    &diagnostic_sources,
                    &file_ids,
                )?;
                self.scope_frame_diagnostics(&diagnostic_sources);
                self.diagnostic_attribution = DiagnosticAttribution::Target;
                let FrameSelection { viewport, .. } = selection;
                // Cycle repairs belong to retained placeholders as well as
                // normalized elements, just like normalization diagnostics.
                (Some(viewport), Some(diagnostic_sources), Some(file_ids))
            }
        };
        Ok(ResolutionInputs {
            normalization,
            relationships,
            frame_viewport,
            frame_participating_sources,
            selected_file_ids,
        })
    }

    /// Phase 2: the semantic-accounting census — asset occurrences, removed
    /// relationship cycles, the working-record reservation, and relationship
    /// edges — charged in the canonical trace order.
    fn account_semantic_census(
        &mut self,
        scene: &SceneDocument,
        request: &RenderRequest,
        inputs: &ResolutionInputs<'_>,
        semantic_working: &mut Vec<SemanticWorkingRecord>,
    ) -> Result<(), RenderError> {
        let ResolutionInputs {
            normalization,
            relationships,
            frame_participating_sources,
            selected_file_ids,
            ..
        } = inputs;
        let occurrence_count = selected_file_ids
            .as_ref()
            .map_or_else(
                || {
                    scene
                        .file_record_count()
                        .checked_add(request.supplemental_assets().len())
                },
                |selected| selected.len().checked_mul(2),
            )
            .ok_or_else(|| self.memory_failure(request, "asset_admission", "occurrence_count"))?;
        let mut asset_occurrence_working = Vec::new();
        asset_occurrence_working
            .try_reserve_exact(occurrence_count)
            .map_err(|_| self.memory_failure(request, "asset_admission", "occurrence_records"))?;
        for index in 0..scene.file_record_count() {
            let Some(record) = scene.file_record_at(index) else {
                continue;
            };
            if selected_file_ids.as_ref().is_some_and(|selected| {
                selected
                    .binary_search_by(|file_id| file_id.as_str().cmp(record.key()))
                    .is_err()
            }) {
                continue;
            }
            let SourceField::Value(data_url) = record.data_url() else {
                continue;
            };
            let decoded_bytes = asset::data_url_decoded_len(data_url);
            let pointer = scene::pointer_child(
                &scene::pointer_child("/files", record.key()).map_err(|_| {
                    self.memory_failure(request, "asset_admission", "scene_asset_pointer")
                })?,
                "dataURL",
            )
            .map_err(|_| self.memory_failure(request, "asset_admission", "scene_asset_pointer"))?;
            let occurrence_bytes = 256_u64.saturating_add(logical_aligned_bytes(
                u64::try_from(if record.key().is_empty() {
                    pointer.len()
                } else {
                    record.key().len()
                })
                .unwrap_or(u64::MAX),
            ));
            self.transact(
                request,
                ResourceContext::new(
                    ResourcePhase::AuthorityReferenceCensus,
                    "asset_occurrence",
                    &pointer,
                )
                .with_local_owner(Some(record.key())),
                ResourceDelta {
                    asset_entries: 1,
                    asset_bytes: decoded_bytes,
                    working_bytes: occurrence_bytes,
                    total_work_units: decoded_bytes,
                    ..ResourceDelta::default()
                },
            )?;
            asset_occurrence_working.push((
                pointer,
                try_owned(record.key()).map_err(|_| {
                    self.memory_failure(request, "asset_admission", "occurrence_owner")
                })?,
                occurrence_bytes,
            ));
        }
        for (file_id, supplemental) in request.supplemental_assets().iter() {
            if selected_file_ids.as_ref().is_some_and(|selected| {
                selected
                    .binary_search_by(|id| id.as_str().cmp(file_id))
                    .is_err()
            }) {
                continue;
            }
            let bytes = u64::try_from(supplemental.bytes().len()).unwrap_or(u64::MAX);
            let occurrence_bytes = 256_u64.saturating_add(logical_aligned_bytes(
                u64::try_from(file_id.len()).unwrap_or(u64::MAX),
            ));
            self.transact(
                request,
                ResourceContext::new(
                    ResourcePhase::AuthorityReferenceCensus,
                    "asset_occurrence",
                    "",
                )
                .with_local_owner(Some(file_id)),
                ResourceDelta {
                    asset_entries: 1,
                    asset_bytes: bytes,
                    working_bytes: occurrence_bytes,
                    total_work_units: bytes.saturating_mul(2),
                    ..ResourceDelta::default()
                },
            )?;
            asset_occurrence_working.push((
                String::new(),
                try_owned(file_id).map_err(|_| {
                    self.memory_failure(request, "asset_admission", "occurrence_owner")
                })?,
                occurrence_bytes,
            ));
        }
        for (pointer, owner, bytes) in &asset_occurrence_working {
            self.release_working(
                request,
                ResourceContext::new(
                    ResourcePhase::AuthorityReferenceCensus,
                    "asset_occurrence",
                    pointer,
                )
                .with_local_owner(Some(owner)),
                *bytes,
            )?;
        }
        for cycle in &relationships.removed_cycles {
            if frame_participating_sources
                .as_ref()
                .is_some_and(|participating| {
                    element_owner_index(&cycle.json_pointer).is_some_and(|source_index| {
                        usize::try_from(source_index)
                            .map(|source_index| participating.binary_search(&source_index).is_err())
                            .unwrap_or(true)
                    })
                })
            {
                continue;
            }
            let (visual, semantic) = self.cycle_effects(cycle, inputs, request);
            let diagnostic = relationship_cycle_diagnostic(
                request.format(),
                &cycle.element_id,
                &cycle.json_pointer,
                visual,
                semantic,
            )
            .map_err(|_| {
                self.memory_failure(request, "compatibility_report", "relationship_cycle")
            })?;
            self.retain_diagnostic_with_owner(
                request,
                ResourcePhase::SharedSemanticResolution,
                "relationship_cycle_removal",
                diagnostic,
                DiagnosticOwnership {
                    local_owner: Some(&cycle.element_id),
                    source_order: Some(cycle.source_order),
                    source_position: Some(cycle.source_position),
                },
            )?;
        }
        let semantic_record_count = normalization
            .elements
            .len()
            .saturating_add(normalization.fallbacks.len())
            .saturating_add(normalization.files.len())
            .saturating_add(relationships.graph.edges.len())
            .saturating_add(1);
        semantic_working
            .try_reserve_exact(semantic_record_count)
            .map_err(|_| self.memory_failure(request, "normalization", "working_records"))?;
        for edge in &relationships.graph.edges {
            let owner = relationships.graph.nodes.get(edge.source).ok_or_else(|| {
                self.internal_failure(
                    request.format(),
                    "relationship edge owner is missing",
                    "/elements",
                )
            })?;
            semantic_working.push(SemanticWorkingRecord {
                source_order: owner.source_order,
                event: "relationship_edge",
                element_id: Some(try_owned(&owner.id).map_err(|_| {
                    self.memory_failure(request, "relationship_resolution", "edge_identity")
                })?),
                local_owner: Some(try_owned(&owner.id).map_err(|_| {
                    self.memory_failure(request, "relationship_resolution", "edge_owner")
                })?),
                json_pointer: try_owned(&edge.json_pointer).map_err(|_| {
                    self.memory_failure(request, "relationship_resolution", "edge_pointer")
                })?,
                bytes: relationships::edge_working_bytes(),
            });
        }
        Ok(())
    }

    /// Phase 4: post-loop accounting and assembly of the resolved
    /// representation, mapping every `ResolveSceneError` onto the
    /// render-error contract.
    #[allow(clippy::too_many_arguments)]
    fn assemble_resolved_scene(
        &mut self,
        scene: &SceneDocument,
        request: &RenderRequest,
        outlines: Vec<(NormalizedElement, Option<GeneratedGeometry>)>,
        images: image::ImageResolver,
        mut fallbacks: Vec<SurfaceFallback>,
        chrome: SceneChrome,
        relationships: &RelationshipOutcome,
        semantic_working: &mut Vec<SemanticWorkingRecord>,
    ) -> Result<ResolvedScene, RenderError> {
        let SceneChrome {
            cjk_subset_trigger,
            files: normalized_files,
            view_background_color,
            view_background_working_bytes,
            frame_rendering: effective_frame_rendering,
            viewport: frame_viewport,
        } = chrome;
        semantic_working.push(SemanticWorkingRecord {
            source_order: scene.source_owner_order("appState", 0),
            event: "normalized_frame_rendering",
            element_id: None,
            local_owner: None,
            json_pointer: "/appState/frameRendering".to_owned(),
            bytes: 4 * 64 + view_background_working_bytes,
        });
        for file in normalized_files {
            semantic_working.push(SemanticWorkingRecord {
                source_order: file.source_order,
                event: "normalized_file_record",
                element_id: None,
                local_owner: Some(file.key),
                json_pointer: file.source_pointer,
                bytes: file.normalized_working_bytes,
            });
        }
        self.account_compatibility_omissions(request)?;
        self.preflight_fallback_text(&mut fallbacks, request, semantic_working)?;
        semantic_working.sort_unstable_by(SemanticWorkingRecord::release_order);
        if outlines.is_empty() && fallbacks.is_empty() {
            match ResolvedScene::empty(&view_background_color, request, frame_viewport) {
                Ok(resolved) => Ok(resolved),
                Err(ResolveSceneError::UnboundedGeometry) => {
                    Err(self.unbounded_geometry_failure(request.format()))
                }
                Err(ResolveSceneError::InvalidColor) => Err(self.internal_failure(
                    request.format(),
                    "normalized scene background color is invalid",
                    "/appState/viewBackgroundColor",
                )),
                Err(
                    ResolveSceneError::Memory
                    | ResolveSceneError::TextOutlines { .. }
                    | ResolveSceneError::FontUnavailable(_)
                    | ResolveSceneError::InvalidFallbackFont
                    | ResolveSceneError::InvalidMandatoryNotice,
                ) => Err(self.internal_failure(
                    request.format(),
                    "empty scene resolution reached an impossible failure",
                    "",
                )),
            }
        } else {
            let mut recoveries = Vec::new();
            let result = ResolvedScene::elements(
                outlines,
                images.into_inputs(),
                fallbacks,
                &relationships.graph,
                effective_frame_rendering,
                &view_background_color,
                request,
                frame_viewport,
                &mut recoveries,
                cjk_subset_trigger,
            );
            match result {
                Ok(Some(mut resolved)) => {
                    resolved.coordinate_omissions = recoveries;
                    Ok(resolved)
                }
                Ok(None) => Err(self.internal_failure(
                    request.format(),
                    "outline semantics are outside the implemented focused case",
                    "/elements",
                )),
                Err(ResolveSceneError::Memory) => {
                    Err(self.memory_failure(request, "semantic_geometry", "outline"))
                }
                Err(ResolveSceneError::FontUnavailable(font)) => {
                    Err(self.font_unavailable_failure(request.format(), font))
                }
                Err(ResolveSceneError::InvalidFallbackFont) => Err(self.internal_failure(
                    request.format(),
                    "the sealed fallback font could not be resolved",
                    "/elements",
                )),
                Err(ResolveSceneError::InvalidMandatoryNotice) => Err(self.format_failure(
                    request,
                    format_error_code(request.format()),
                    FormatEncodeError::Encoding(
                        "the mandatory font notice could not be resolved".to_owned(),
                    ),
                )),
                Err(ResolveSceneError::InvalidColor) => Err(self.internal_failure(
                    request.format(),
                    "normalized scene color is invalid",
                    "/elements",
                )),
                Err(ResolveSceneError::UnboundedGeometry) => {
                    Err(self.unbounded_geometry_failure(request.format()))
                }
                Err(ResolveSceneError::TextOutlines {
                    geometry_segments,
                    element_id,
                    json_pointer,
                }) => {
                    // The kept outlines alone exceed the total geometry limit,
                    // so charging them crosses it with its ordinary failure.
                    self.transact(
                        request,
                        ResourceContext::new(
                            ResourcePhase::SharedSemanticResolution,
                            "text_outline_preflight",
                            &json_pointer,
                        )
                        .with_element_id(Some(&element_id))
                        .with_local_owner(Some(&element_id)),
                        ResourceDelta {
                            geometry_segments,
                            ..ResourceDelta::default()
                        },
                    )?;
                    Err(self.internal_failure(
                        request.format(),
                        "text outlines past the total geometry limit were admitted",
                        "/elements",
                    ))
                }
            }
        }
    }
}
