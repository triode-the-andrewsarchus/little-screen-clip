//! Image-element resolution: the deepest single mass of shared semantic
//! resolution, behind a three-method interface.
//!
//! Everything an image element needs — asset resolution through the private
//! [`asset::Cache`], the intrinsic-transaction accounting protocol, claims
//! validation, unsafe-SVG and animation diagnostics, fallback classification,
//! and the sanitized-tree deposit into the session cache — happens behind
//! [`ImageResolver::resolve_element`]. A maintainer changing image semantics
//! edits this file; `resolve_scene` only routes elements and dispositions.
//!
//! The implementation is a verbatim move out of `resolve_scene`: every
//! `session.transact` fires in the same order with the same context strings,
//! so artifact bytes and the canonical resource trace are unchanged.

use crate::normalization::{NormalizedElement, NormalizedElementKind};
use crate::resource_accounting::{ResourceContext, ResourceDelta, ResourcePhase};
use crate::{
    DiagnosticCode, DiagnosticDetails, OutputFormat, RenderError, RenderRequest, SceneDocument,
};

use super::diagnostics::{
    diagnostic_uses_non_generic_recovery, image_animation_flattened_diagnostic,
    image_asset_failure_diagnostic, image_payload_field_recovery, nested_image_failure_diagnostic,
    nested_media_claim_diagnostic, unsafe_svg_removal_diagnostic,
};
use super::resolve::{
    asset_claim_location, asset_failure_location, logical_aligned_bytes, semantic_identity_hex,
    ResolvedImageAsset, ResolvedImageInputs, SemanticWorkingRecord,
};
use super::{asset, try_owned, DiagnosticOwnership, PendingAnimationDiagnostic, RenderSession};

/// What became of one image element.
pub(super) enum ImageDisposition {
    /// The element is fully disposed — a resolved asset or a recorded image
    /// fallback. The caller pushes `(element, None)` into the outlines and
    /// moves to the next element.
    Outline(NormalizedElement),
    /// No image-specific disposition applies (no file id, or the failure
    /// routes through the shared known-element path). The caller continues
    /// with the generic handling for the returned element.
    Generic(NormalizedElement),
}

/// Resolves image elements for one render, owning the asset cache and the
/// image accumulators that feed [`ResolvedImageInputs`].
///
/// Invariants: call [`Self::resolve_element`] once per element in ascending
/// source order — charge order and diagnostic retention order are part of the
/// compatibility contract. Call [`Self::into_inputs`] exactly once, after the
/// element loop. Each resolved source index lands in the assets map or the
/// fallback list, never both.
pub(super) struct ImageResolver {
    authorities: Vec<(usize, asset::Authority)>,
    cache: asset::Cache,
    assets: std::collections::BTreeMap<usize, std::sync::Arc<ResolvedImageAsset>>,
    fallbacks: Vec<(usize, crate::normalization::NormalizedImageStatus)>,
    raster_limit_owners: Vec<String>,
    forbidden_owners: Vec<(String, bool)>,
}

impl ImageResolver {
    pub(super) fn new(authorities: Vec<(usize, asset::Authority)>) -> Self {
        Self {
            authorities,
            cache: asset::Cache::default(),
            assets: std::collections::BTreeMap::new(),
            fallbacks: Vec::new(),
            raster_limit_owners: Vec::new(),
            forbidden_owners: Vec::new(),
        }
    }

    /// Resolves one element. Non-image elements come back untouched as
    /// [`ImageDisposition::Generic`]. Asset-level failures are dispositions,
    /// not errors; `Err` is render-fatal with accounting already charged.
    pub(super) fn resolve_element(
        &mut self,
        session: &mut RenderSession,
        scene: &SceneDocument,
        request: &RenderRequest,
        mut source: NormalizedElement,
        semantic_working: &mut Vec<SemanticWorkingRecord>,
    ) -> Result<ImageDisposition, RenderError> {
        let NormalizedElementKind::Image(image) = &source.kind else {
            return Ok(ImageDisposition::Generic(source));
        };
        let mut raster_limit_recovery = false;
        if let Some(file_id) = image.file_id.as_deref() {
            let authority = self
                .authorities
                .binary_search_by_key(&source.source.source_index(), |(index, _)| *index)
                .ok()
                .and_then(|index| self.authorities.get(index))
                .map_or(asset::Authority::Missing, |(_, authority)| *authority);
            let mut pending_asset_working = Vec::new();
            pending_asset_working.try_reserve_exact(2).map_err(|_| {
                session.memory_failure(request, "asset_resolution", "pending_working_records")
            })?;
            let asset_resolution = self.cache.resolve_with_viewport(
                scene,
                request.supplemental_assets(),
                session.profile.compatibility_revision(),
                request.format(),
                request.resource_limits(),
                file_id,
                authority,
                Some((source.width, source.height)),
                |transaction| match transaction {
                    asset::IntrinsicTransaction::PreflightNested {
                        asset_entries,
                        asset_bytes,
                        decoded_pixels,
                        total_work_units,
                    } => {
                        let delta = ResourceDelta {
                            asset_entries,
                            asset_bytes,
                            decoded_pixels,
                            total_work_units,
                            ..ResourceDelta::default()
                        };
                        if session.meter.would_cross_fatal(delta) {
                            session.transact(
                                request,
                                ResourceContext::new(
                                    ResourcePhase::IntrinsicAssets,
                                    "nested_asset_preflight",
                                    source.source.pointer(),
                                )
                                .with_local_owner(Some(file_id)),
                                delta,
                            )?;
                            return Err(session.internal_failure(
                                request.format(),
                                "fatal nested asset preflight committed unexpectedly",
                                "/elements",
                            ));
                        }
                        Ok(())
                    }
                    asset::IntrinsicTransaction::DiscoverNestedOccurrence { asset_bytes } => {
                        session.transact(
                            request,
                            ResourceContext::new(
                                ResourcePhase::IntrinsicAssets,
                                "nested_asset_occurrence",
                                source.source.pointer(),
                            )
                            .with_local_owner(Some(file_id)),
                            ResourceDelta {
                                asset_entries: 1,
                                asset_bytes,
                                total_work_units: asset_bytes,
                                ..ResourceDelta::default()
                            },
                        )
                    }
                    asset::IntrinsicTransaction::DiscoverNestedPixels { decoded_pixels } => session
                        .transact(
                            request,
                            ResourceContext::new(
                                ResourcePhase::IntrinsicAssets,
                                "nested_asset_decode",
                                source.source.pointer(),
                            )
                            .with_local_owner(Some(file_id)),
                            ResourceDelta {
                                decoded_pixels,
                                ..ResourceDelta::default()
                            },
                        ),
                    asset::IntrinsicTransaction::RejectLocalWork {
                        event,
                        identity,
                        total_work_units,
                    } => {
                        let identity = semantic_identity_hex(&identity).map_err(|_| {
                            session.memory_failure(request, "asset_resolution", "semantic_identity")
                        })?;
                        session.preflight_local_rejection(
                            request,
                            ResourceContext::new(
                                ResourcePhase::IntrinsicAssets,
                                event,
                                source.source.pointer(),
                            )
                            .with_semantic_identity(&identity)
                            .with_local_owner(Some(file_id)),
                            ResourceDelta {
                                total_work_units,
                                ..ResourceDelta::default()
                            },
                        )
                    }
                    asset::IntrinsicTransaction::Acquire {
                        event,
                        identity,
                        persistent_working_bytes,
                        temporary_working_bytes,
                        decoded_pixels,
                        total_work_units,
                    } => {
                        let identity = semantic_identity_hex(&identity).map_err(|_| {
                            session.memory_failure(request, "asset_resolution", "semantic_identity")
                        })?;
                        let record = (persistent_working_bytes != 0)
                            .then(|| {
                                Ok::<_, RenderError>(SemanticWorkingRecord {
                                    source_order: source.source_order,
                                    event,
                                    element_id: None,
                                    local_owner: None,
                                    json_pointer: try_owned(source.source.pointer()).map_err(
                                        |_| {
                                            session.memory_failure(
                                                request,
                                                "asset_resolution",
                                                "semantic_working_pointer",
                                            )
                                        },
                                    )?,
                                    bytes: persistent_working_bytes,
                                })
                            })
                            .transpose()?;
                        session.transact(
                            request,
                            ResourceContext::new(
                                ResourcePhase::IntrinsicAssets,
                                event,
                                source.source.pointer(),
                            )
                            .with_semantic_identity(&identity)
                            .with_sharing_key(event),
                            ResourceDelta {
                                decoded_pixels,
                                working_bytes: persistent_working_bytes
                                    .saturating_add(temporary_working_bytes),
                                total_work_units,
                                ..ResourceDelta::default()
                            },
                        )?;
                        if let Some(record) = record {
                            pending_asset_working.push(record);
                        }
                        Ok(())
                    }
                    asset::IntrinsicTransaction::ReleaseTemporary {
                        event,
                        identity,
                        bytes,
                    } => {
                        let identity = semantic_identity_hex(&identity).map_err(|_| {
                            session.memory_failure(request, "asset_resolution", "semantic_identity")
                        })?;
                        session.release_working(
                            request,
                            ResourceContext::new(
                                ResourcePhase::IntrinsicAssets,
                                event,
                                source.source.pointer(),
                            )
                            .with_semantic_identity(&identity),
                            bytes,
                        )
                    }
                    asset::IntrinsicTransaction::Abort {
                        event,
                        identity,
                        bytes,
                    } => {
                        let identity = semantic_identity_hex(&identity).map_err(|_| {
                            session.memory_failure(request, "asset_resolution", "semantic_identity")
                        })?;
                        session.release_working(
                            request,
                            ResourceContext::new(
                                ResourcePhase::IntrinsicAssets,
                                event,
                                source.source.pointer(),
                            )
                            .with_semantic_identity(&identity),
                            bytes,
                        )?;
                        pending_asset_working.retain(|record| record.event != event);
                        Ok(())
                    }
                },
            );
            let asset_resolution = match asset_resolution {
                Ok(resolution) => Ok(resolution),
                Err(asset::ResolutionFailure::Asset(error)) => Err(error),
                Err(asset::ResolutionFailure::Transaction(error)) => return Err(error),
                Err(asset::ResolutionFailure::Memory) => {
                    return Err(session.memory_failure(
                        request,
                        "nested_asset_discovery",
                        "occurrence_cache",
                    ));
                }
            };
            semantic_working.append(&mut pending_asset_working);
            let asset_failure = asset_resolution.as_ref().err().copied();
            if let Ok(resolution) = asset_resolution {
                let forbidden_root = resolution.svg_failures.iter().any(|failure| {
                    failure.error == asset::ResolveError::Forbidden && failure.path.is_empty()
                });
                let animated = resolution.animated;
                let media_type = resolution.media_kind.media_type();
                let svg_diagnostic_location = if resolution.svg_removals.is_empty()
                    && resolution.svg_failures.is_empty()
                    && resolution.svg_media_claims.is_empty()
                {
                    None
                } else {
                    Some(
                        asset_failure_location(
                            scene,
                            request.supplemental_assets(),
                            file_id,
                            authority,
                        )
                        .map_err(|_| {
                            session.memory_failure(
                                request,
                                "compatibility_report",
                                "unsafe_svg_location",
                            )
                        })?,
                    )
                };
                for removal in &resolution.svg_removals {
                    let (json_pointer, source_order, source_position, local_owner) =
                        svg_diagnostic_location.as_ref().ok_or_else(|| {
                            session.internal_failure(
                                request.format(),
                                "unsafe SVG diagnostic lost its source location",
                                "/files",
                            )
                        })?;
                    let diagnostic = unsafe_svg_removal_diagnostic(
                        request.format(),
                        file_id,
                        json_pointer,
                        removal,
                    )
                    .map_err(|_| {
                        session.memory_failure(
                            request,
                            "compatibility_report",
                            "unsafe_svg_removal",
                        )
                    })?;
                    session.retain_diagnostic_with_owner(
                        request,
                        ResourcePhase::SharedSemanticResolution,
                        "unsafe_svg_removal",
                        diagnostic,
                        DiagnosticOwnership {
                            local_owner: Some(local_owner),
                            source_order: Some(*source_order),
                            source_position: Some(*source_position),
                        },
                    )?;
                }
                for failure in &resolution.svg_failures {
                    if failure.error == asset::ResolveError::Forbidden && failure.path.is_empty() {
                        // One file diagnostic, but each selected reference gets
                        // its own viewport. Aggregate only actual target-local
                        // replacements, not deleted or out-of-target bounds.
                        let existing = self
                            .forbidden_owners
                            .iter()
                            .position(|(id, _)| id == file_id);
                        let paints = failure.placeholder
                            || existing.is_some_and(|i| self.forbidden_owners[i].1);
                        let update=if let Some(diagnostic) = session.report.diagnostics.iter_mut().find(|d| d.code() == DiagnosticCode::IMAGE_FORBIDDEN_RESOURCE && matches!(d.details(),Some(DiagnosticDetails::Asset {asset_id,..}) if asset_id == file_id)) {
                            diagnostic.set_forbidden_resource_recovery(paints)
                        }else{Ok(())};
                        update.map_err(|_| {
                            session.memory_failure(
                                request,
                                "compatibility_report",
                                "forbidden_recipe",
                            )
                        })?;
                        if let Some(i) = existing {
                            self.forbidden_owners[i].1 = paints;
                        } else {
                            self.forbidden_owners.try_reserve(1).map_err(|_| {
                                session.memory_failure(
                                    request,
                                    "asset_resolution",
                                    "forbidden_owner",
                                )
                            })?;
                            self.forbidden_owners.push((
                                try_owned(file_id).map_err(|_| {
                                    session.memory_failure(
                                        request,
                                        "asset_resolution",
                                        "forbidden_owner",
                                    )
                                })?,
                                paints,
                            ));
                        }
                        continue;
                    }
                    let (json_pointer, source_order, source_position, local_owner) =
                        svg_diagnostic_location.as_ref().ok_or_else(|| {
                            session.internal_failure(
                                request.format(),
                                "nested SVG failure lost its source location",
                                "/files",
                            )
                        })?;
                    let diagnostic = nested_image_failure_diagnostic(
                        failure,
                        request.format(),
                        file_id,
                        json_pointer,
                        request.resource_limits(),
                    )
                    .map_err(|_| {
                        session.memory_failure(
                            request,
                            "compatibility_report",
                            "nested_image_failure",
                        )
                    })?;
                    session.retain_diagnostic_with_owner(
                        request,
                        ResourcePhase::SharedSemanticResolution,
                        "nested_image_failure",
                        diagnostic,
                        DiagnosticOwnership {
                            local_owner: Some(local_owner),
                            source_order: Some(*source_order),
                            source_position: Some(*source_position),
                        },
                    )?;
                }
                for claim in &resolution.svg_media_claims {
                    let (pointer, source_order, source_position, local_owner) =
                        svg_diagnostic_location.as_ref().ok_or_else(|| {
                            session.internal_failure(
                                request.format(),
                                "nested media claim lost its source location",
                                "/files",
                            )
                        })?;
                    let diagnostic =
                        nested_media_claim_diagnostic(request.format(), file_id, pointer, claim)
                            .map_err(|_| {
                                session.memory_failure(
                                    request,
                                    "compatibility_report",
                                    "nested_media_claim",
                                )
                            })?;
                    session.retain_diagnostic_with_owner(
                        request,
                        ResourcePhase::SharedSemanticResolution,
                        "nested_media_claim",
                        diagnostic,
                        DiagnosticOwnership {
                            local_owner: Some(local_owner),
                            source_order: Some(*source_order),
                            source_position: Some(*source_position),
                        },
                    )?;
                }
                let asset = resolution.asset;
                if let Some(tree) = resolution.svg_tree {
                    session.svg_trees.insert(asset.identity, tree);
                }
                // Strict-domain normalization already diagnosed malformed scene
                // MIME values before payload validation. Enrich that occurrence
                // rather than retaining a second warning (or leaving its type
                // unknown after successful validation). Its source order and
                // diagnostic resource charge remain those of normalization.
                if !forbidden_root && matches!(authority, asset::Authority::Scene) {
                    let (pointer, _, _, _) = asset_claim_location(
                        scene,
                        request.supplemental_assets(),
                        file_id,
                        authority,
                        "mimeType",
                    )
                    .map_err(|_| {
                        session.memory_failure(
                            request,
                            "compatibility_report",
                            "asset_claim_pointer",
                        )
                    })?;
                    for index in 0..session.report.diagnostics.len() {
                        let diagnostic = &session.report.diagnostics[index];
                        if diagnostic.code() != DiagnosticCode::ASSET_MEDIA_TYPE_MISMATCH
                            || diagnostic.json_pointer() != pointer
                        {
                            continue;
                        }
                        if matches!(diagnostic.details(), Some(DiagnosticDetails::Asset { asset_id, validated_media_type: None, .. }) if asset_id == file_id)
                        {
                            let validated = try_owned(media_type).map_err(|_| {
                                session.memory_failure(
                                    request,
                                    "compatibility_report",
                                    "validated_media_type",
                                )
                            })?;
                            if let Some(DiagnosticDetails::Asset {
                                validated_media_type,
                                ..
                            }) = session.report.diagnostics[index].details_mut()
                            {
                                *validated_media_type = Some(validated);
                            }
                        }
                    }
                }
                for (claim, field) in [
                    (resolution.claims.mime_type.as_deref(), "mimeType"),
                    (resolution.claims.data_url_media_type.as_deref(), "dataURL"),
                ] {
                    let Some(claim) = claim.filter(|claim| {
                        !claim
                            .split(';')
                            .next()
                            .unwrap_or_default()
                            .trim()
                            .eq_ignore_ascii_case(media_type)
                    }) else {
                        continue;
                    };
                    let (json_pointer, source_order, source_position, local_owner) =
                        asset_claim_location(
                            scene,
                            request.supplemental_assets(),
                            file_id,
                            authority,
                            field,
                        )
                        .map_err(|_| {
                            session.memory_failure(request, "compatibility_report", "asset_pointer")
                        })?;
                    // Claims belong to the authoritative file/request field,
                    // not each image that references it. Distinct fields and
                    // file IDs remain distinct even when their bytes are shared.
                    if session.report.diagnostics.iter().any(|diagnostic| {
                        diagnostic.code() == DiagnosticCode::ASSET_MEDIA_TYPE_MISMATCH
                            && diagnostic.json_pointer() == json_pointer
                            && matches!(diagnostic.details(), Some(DiagnosticDetails::Asset {
                                asset_id, claimed_media_type, validated_media_type, ..
                            }) if asset_id == file_id
                                && claimed_media_type.as_deref() == Some(claim)
                                && validated_media_type.as_deref() == Some(media_type))
                    }) {
                        continue;
                    }
                    let diagnostic = crate::scene_surface::asset_media_type_diagnostic(
                        request.format(),
                        file_id,
                        &json_pointer,
                        Some(claim),
                        Some(media_type),
                    )
                    .map_err(|_| {
                        session.memory_failure(
                            request,
                            "compatibility_report",
                            "asset_media_type_mismatch",
                        )
                    })?;
                    session.retain_diagnostic_with_owner(
                        request,
                        ResourcePhase::SharedSemanticResolution,
                        "asset_media_type_mismatch",
                        diagnostic,
                        DiagnosticOwnership {
                            local_owner: Some(&local_owner),
                            source_order: Some(source_order),
                            source_position: Some(source_position),
                        },
                    )?;
                }
                if animated && request.format() != OutputFormat::Svg {
                    let file_id_pointer =
                        crate::scene::pointer_child(source.source.pointer(), "fileId").map_err(
                            |_| {
                                session.memory_failure(
                                    request,
                                    "compatibility_report",
                                    "image_animation_pointer",
                                )
                            },
                        )?;
                    if request.format() == OutputFormat::Png {
                        let pointer_bytes =
                            u64::try_from(file_id_pointer.len()).unwrap_or(u64::MAX);
                        let owner_bytes =
                            u64::try_from(source.source.id().len()).unwrap_or(u64::MAX);
                        let working_bytes = 512_u64
                            .saturating_add(logical_aligned_bytes(pointer_bytes))
                            .saturating_add(logical_aligned_bytes(owner_bytes));
                        session.acquire_working(
                            request,
                            ResourceContext::new(
                                ResourcePhase::SharedSemanticResolution,
                                "pending_image_animation_flattened",
                                &file_id_pointer,
                            )
                            .with_element_id(Some(source.source.id()))
                            .with_local_owner(Some(source.source.id())),
                            working_bytes,
                        )?;
                        session
                            .pending_png_animation_diagnostics
                            .try_reserve(1)
                            .map_err(|_| {
                                session.memory_failure(
                                    request,
                                    "compatibility_report",
                                    "pending_image_animation_flattened",
                                )
                            })?;
                        let local_owner = try_owned(source.source.id()).map_err(|_| {
                            session.memory_failure(
                                request,
                                "compatibility_report",
                                "pending_image_animation_owner",
                            )
                        })?;
                        let diagnostic = image_animation_flattened_diagnostic(
                            request.format(),
                            file_id,
                            source.source.id(),
                            &file_id_pointer,
                            media_type,
                        )
                        .map_err(|_| {
                            session.memory_failure(
                                request,
                                "compatibility_report",
                                "image_animation_flattened",
                            )
                        })?;
                        session.pending_png_animation_diagnostics.push(
                            PendingAnimationDiagnostic {
                                diagnostic,
                                local_owner,
                                source_order: source.source_order,
                                working_bytes,
                            },
                        );
                    } else {
                        let diagnostic = image_animation_flattened_diagnostic(
                            request.format(),
                            file_id,
                            source.source.id(),
                            &file_id_pointer,
                            media_type,
                        )
                        .map_err(|_| {
                            session.memory_failure(
                                request,
                                "compatibility_report",
                                "image_animation_flattened",
                            )
                        })?;
                        session.retain_diagnostic_with_owner(
                            request,
                            ResourcePhase::SharedSemanticResolution,
                            "image_animation_flattened",
                            diagnostic,
                            DiagnosticOwnership {
                                local_owner: Some(source.source.id()),
                                source_order: Some(source.source_order),
                                source_position: Some(u64::MAX),
                            },
                        )?;
                    }
                }
                if forbidden_root {
                    // The replacement already fills the normalized local
                    // viewport. A crop of the unavailable payload must not
                    // crop the new recipe a second time; flips, rotation and
                    // existing corner/frame clips still apply to that viewport.
                    if let NormalizedElementKind::Image(image) = &mut source.kind {
                        image.crop = None;
                    }
                }
                self.assets.insert(source.source.source_index(), asset);
                semantic_working.push(SemanticWorkingRecord {
                    source_order: source.source_order,
                    event: "resolved_image",
                    element_id: Some(try_owned(source.source.id()).map_err(|_| {
                        session.memory_failure(request, "normalization", "working_record_identity")
                    })?),
                    local_owner: Some(try_owned(source.source.id()).map_err(|_| {
                        session.memory_failure(request, "normalization", "working_record_owner")
                    })?),
                    json_pointer: try_owned(source.source.pointer()).map_err(|_| {
                        session.memory_failure(request, "normalization", "working_record_pointer")
                    })?,
                    bytes: source.normalized_working_bytes,
                });
                return Ok(ImageDisposition::Outline(source));
            }
            if let Some(failure) = asset_failure {
                raster_limit_recovery = matches!(
                    failure,
                    asset::ResolveError::Dimensions { .. } | asset::ResolveError::Pixels { .. }
                );
                if matches!(failure, asset::ResolveError::Memory) {
                    return Err(session.memory_failure(
                        request,
                        "asset_resolution",
                        "raster_asset",
                    ));
                }
                if !matches!(failure, asset::ResolveError::Missing)
                    && !(raster_limit_recovery
                        && self
                            .raster_limit_owners
                            .iter()
                            .any(|owner| owner == file_id))
                    && !session.report.diagnostics.iter().any(|diagnostic| {
                        (matches!(
                            diagnostic.details(),
                            Some(DiagnosticDetails::Asset { asset_id, .. })
                                if asset_id == file_id
                        ) && diagnostic.replaces_occurrence())
                            || image_payload_field_recovery(&source, diagnostic)
                    })
                {
                    let (json_pointer, source_order, source_position, local_owner) =
                        asset_failure_location(
                            scene,
                            request.supplemental_assets(),
                            file_id,
                            authority,
                        )
                        .map_err(|_| {
                            session.memory_failure(
                                request,
                                "compatibility_report",
                                "asset_failure_pointer",
                            )
                        })?;
                    let diagnostic = image_asset_failure_diagnostic(
                        failure,
                        request.format(),
                        file_id,
                        source.source.id(),
                        &json_pointer,
                        image.status,
                        request.resource_limits(),
                    )
                    .map_err(|_| {
                        session.memory_failure(request, "compatibility_report", "asset_failure")
                    })?;
                    session.retain_diagnostic_with_owner(
                        request,
                        ResourcePhase::SharedSemanticResolution,
                        "asset_failure",
                        diagnostic,
                        DiagnosticOwnership {
                            local_owner: Some(&local_owner),
                            source_order: Some(source_order),
                            source_position: Some(source_position),
                        },
                    )?;
                    if raster_limit_recovery {
                        self.raster_limit_owners.try_reserve(1).map_err(|_| {
                            session.memory_failure(
                                request,
                                "compatibility_report",
                                "raster_limit_owner",
                            )
                        })?;
                        self.raster_limit_owners
                            .push(try_owned(file_id).map_err(|_| {
                                session.memory_failure(
                                    request,
                                    "compatibility_report",
                                    "raster_limit_owner",
                                )
                            })?);
                    }
                }
            }
        }
        if request.format() == OutputFormat::Png
            && (raster_limit_recovery
                || image.file_id.is_none()
                || session
                    .report
                    .diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic_uses_non_generic_recovery(&source, diagnostic)))
        {
            self.fallbacks.try_reserve(1).map_err(|_| {
                session.memory_failure(request, "asset_resolution", "image_fallbacks")
            })?;
            self.fallbacks
                .push((source.source.source_index(), image.status));
            semantic_working.push(SemanticWorkingRecord {
                source_order: source.source_order,
                event: "resolved_image_fallback",
                element_id: Some(try_owned(source.source.id()).map_err(|_| {
                    session.memory_failure(request, "normalization", "working_record_identity")
                })?),
                local_owner: Some(try_owned(source.source.id()).map_err(|_| {
                    session.memory_failure(request, "normalization", "working_record_owner")
                })?),
                json_pointer: try_owned(source.source.pointer()).map_err(|_| {
                    session.memory_failure(request, "normalization", "working_record_pointer")
                })?,
                bytes: source.normalized_working_bytes,
            });
            return Ok(ImageDisposition::Outline(source));
        }
        // An invalid fileId is normalized to null and already diagnosed at
        // that field. It has the same format-specific absent-image paint as an
        // explicit null, rather than replacing the whole image element.
        if image.file_id.is_none() || raster_limit_recovery {
            semantic_working.try_reserve(1).map_err(|_| {
                session.memory_failure(request, "asset_resolution", "image_omission")
            })?;
            semantic_working.push(SemanticWorkingRecord {
                source_order: source.source_order,
                event: "resolved_image_fallback",
                element_id: Some(try_owned(source.source.id()).map_err(|_| {
                    session.memory_failure(request, "normalization", "working_record_identity")
                })?),
                local_owner: Some(try_owned(source.source.id()).map_err(|_| {
                    session.memory_failure(request, "normalization", "working_record_owner")
                })?),
                json_pointer: try_owned(source.source.pointer()).map_err(|_| {
                    session.memory_failure(request, "normalization", "working_record_pointer")
                })?,
                bytes: source.normalized_working_bytes,
            });
            return Ok(ImageDisposition::Outline(source));
        }
        Ok(ImageDisposition::Generic(source))
    }

    /// The image inputs the resolved representation is assembled from.
    pub(super) fn into_inputs(self) -> ResolvedImageInputs {
        ResolvedImageInputs {
            assets: self.assets,
            fallbacks: self.fallbacks,
        }
    }
}
