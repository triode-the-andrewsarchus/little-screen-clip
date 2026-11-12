//! The frame-batch journal. A [`PreparedFrameRender`] records the original-JSON
//! traversal, normalization, and relationship events one scene produces so a
//! batch of frame exports can replay them into each frame's session instead of
//! recomputing them, and the session's observers that ingest those events —
//! live or replayed — charge the meter identically either way. The frame batch
//! in `frame.rs` constructs a journal and hands it to `render_frame_scoped`,
//! which replays it into each frame's session.

use std::sync::{Arc, Mutex};

use crate::normalization::{NormalizationError, NormalizationEvent, NormalizationOutcome};
use crate::outcome::PendingDiagnostic;
use crate::relationships::{
    self, RelationshipError, RelationshipEvent, RelationshipOutcome, RelationshipTraversal,
};
use crate::resource_accounting::{ResourceContext, ResourceDelta, ResourcePhase};
use crate::scene::{OriginalJsonEntryKind, OriginalJsonVisitError};
use crate::scene_surface::SceneSurfacePolicy;
use crate::{ReferenceProfileIdentity, RenderError, RenderRequest, SceneDocument};

use super::frame_target::{selected_file_ids, FrameSelection, FrameTarget};
use super::{
    svg_fonts, try_owned, DiagnosticOwnership, PendingNormalizationDiagnostic, RenderSession,
    RenderTarget, SharedFontSubsetSession,
};

pub(crate) struct PreparedFrameRender {
    pub(super) original_json_events: Vec<PreparedOriginalJsonEvent>,
    pub(super) normalization_events: Vec<PreparedNormalizationEvent>,
    pub(super) normalization: NormalizationOutcome,
    pub(super) relationship_events: Vec<PreparedRelationshipEvent>,
    pub(super) relationships: RelationshipOutcome,
    pub(super) font_subset_session: SharedFontSubsetSession,
}

pub(crate) struct FramePreparationError {
    subject: &'static str,
}

impl FramePreparationError {
    const fn failed(subject: &'static str) -> Self {
        Self { subject }
    }

    pub(crate) const fn subject(&self) -> &'static str {
        self.subject
    }
}

pub(super) struct PreparedOriginalJsonEvent {
    pub(super) kind: OriginalJsonEntryKind,
    pub(super) json_pointer: String,
    pub(super) is_element_occurrence: bool,
}

pub(super) enum PreparedNormalizationEvent {
    PreflightRecord {
        event: &'static str,
        json_pointer: String,
        working_bytes: u64,
        work_units: u64,
    },
    CommitRecord {
        event: &'static str,
        element_id: Option<String>,
        local_owner: Option<String>,
        json_pointer: String,
        working_bytes: u64,
        work_units: u64,
    },
    Diagnostic {
        diagnostic: PendingDiagnostic,
        local_owner: Option<String>,
        source_order: u64,
        source_position: u64,
    },
}

pub(super) enum PreparedRelationshipEvent {
    SearchStep {
        element_id: String,
        json_pointer: String,
        attempted_depth: u32,
    },
    Traversal {
        element_id: String,
        local_owner: String,
        json_pointer: String,
        attempted_depth: u32,
        retained: bool,
    },
}

impl PreparedFrameRender {
    pub(crate) fn new(
        profile: &'static ReferenceProfileIdentity,
        scene_surface: &'static SceneSurfacePolicy,
        scene: &SceneDocument,
        request: &RenderRequest,
    ) -> Result<Self, FramePreparationError> {
        let mut original_json_events = Vec::new();
        let traversal = scene.visit_original_json_entries(|entry| {
            original_json_events
                .try_reserve(1)
                .map_err(|_| FramePreparationError::failed("original_json_journal"))?;
            original_json_events.push(PreparedOriginalJsonEvent {
                kind: entry.kind,
                json_pointer: try_owned(entry.json_pointer)
                    .map_err(|_| FramePreparationError::failed("original_json_journal"))?,
                is_element_occurrence: entry.is_element_occurrence,
            });
            Ok(())
        });
        match traversal {
            Ok(()) => {}
            Err(OriginalJsonVisitError::Visitor(error)) => return Err(error),
            Err(OriginalJsonVisitError::MemoryUnavailable { .. }) => {
                return Err(FramePreparationError::failed("original_json_traversal"));
            }
        }

        let mut normalization_events = Vec::new();
        let normalization = crate::normalization::normalize_scene_with_assets(
            scene,
            profile.compatibility_revision(),
            scene_surface,
            request.format(),
            request.supplemental_assets(),
            false,
            usize::MAX,
            |event| PreparedNormalizationEvent::record(&mut normalization_events, event),
        );
        let normalization = match normalization {
            Ok(Some(normalization)) => normalization,
            Ok(None) => return Err(FramePreparationError::failed("normalization_outcome")),
            Err(NormalizationError::Observer(error)) => return Err(error),
            Err(NormalizationError::MemoryUnavailable { .. }) => {
                return Err(FramePreparationError::failed("normalization"));
            }
        };

        let mut relationship_events = Vec::new();
        let relationships = relationships::resolve(
            scene,
            &normalization.elements,
            request.supplemental_assets(),
            |event| PreparedRelationshipEvent::record(&mut relationship_events, event),
        );
        let relationships = match relationships {
            Ok(relationships) => relationships,
            Err(RelationshipError::Observer(error)) => return Err(error),
            Err(RelationshipError::MemoryUnavailable) => {
                return Err(FramePreparationError::failed("relationship_resolution"));
            }
        };

        Ok(Self {
            original_json_events,
            normalization_events,
            normalization,
            relationship_events,
            relationships,
            font_subset_session: Arc::new(Mutex::new(svg_fonts::Session::new())),
        })
    }

    pub(super) fn normalization_for_target(
        &self,
        target: FrameTarget,
    ) -> Result<NormalizationOutcome, std::collections::TryReserveError> {
        let Some(selection) = FrameSelection::compute(
            &self.normalization.elements,
            &self.relationships.graph,
            target,
        )?
        else {
            return Ok(self.normalization.clone());
        };
        let mut elements = Vec::new();
        elements.try_reserve_exact(selection.participating.len())?;
        elements.extend(
            self.normalization
                .elements
                .iter()
                .filter(|element| selection.retains_element(element))
                .cloned(),
        );
        let mut fallbacks = Vec::new();
        fallbacks.try_reserve_exact(self.normalization.fallbacks.len())?;
        fallbacks.extend(
            self.normalization
                .fallbacks
                .iter()
                .filter(|fallback| selection.retains_fallback(fallback))
                .cloned(),
        );
        let file_ids = selected_file_ids(&elements);
        let mut files = Vec::new();
        files.try_reserve_exact(self.normalization.files.len())?;
        files.extend(
            self.normalization
                .files
                .iter()
                .filter(|file| file_ids.binary_search(&file.key).is_ok())
                .cloned(),
        );
        Ok(NormalizationOutcome {
            elements,
            fallbacks,
            view_background_color: self.normalization.view_background_color.clone(),
            view_background_working_bytes: self.normalization.view_background_working_bytes,
            frame_rendering: self.normalization.frame_rendering,
            files,
        })
    }
}

impl PreparedNormalizationEvent {
    fn record(
        events: &mut Vec<Self>,
        event: NormalizationEvent<'_>,
    ) -> Result<(), FramePreparationError> {
        let event = match event {
            NormalizationEvent::PreflightRecord {
                event,
                json_pointer,
                working_bytes,
                work_units,
            } => Self::PreflightRecord {
                event,
                json_pointer: try_owned(json_pointer)
                    .map_err(|_| FramePreparationError::failed("normalization_journal"))?,
                working_bytes,
                work_units,
            },
            NormalizationEvent::CommitRecord {
                event,
                element_id,
                local_owner,
                json_pointer,
                working_bytes,
                work_units,
            } => Self::CommitRecord {
                event,
                element_id: element_id
                    .map(try_owned)
                    .transpose()
                    .map_err(|_| FramePreparationError::failed("normalization_journal"))?,
                local_owner: local_owner
                    .map(try_owned)
                    .transpose()
                    .map_err(|_| FramePreparationError::failed("normalization_journal"))?,
                json_pointer: try_owned(json_pointer)
                    .map_err(|_| FramePreparationError::failed("normalization_journal"))?,
                working_bytes,
                work_units,
            },
            NormalizationEvent::Diagnostic {
                diagnostic,
                local_owner,
                source_order,
                source_position,
            } => Self::Diagnostic {
                diagnostic,
                local_owner: local_owner
                    .map(try_owned)
                    .transpose()
                    .map_err(|_| FramePreparationError::failed("normalization_journal"))?,
                source_order,
                source_position,
            },
        };
        events
            .try_reserve(1)
            .map_err(|_| FramePreparationError::failed("normalization_journal"))?;
        events.push(event);
        Ok(())
    }

    pub(super) fn replay(
        &self,
        session: &mut RenderSession,
        request: &RenderRequest,
    ) -> Result<(), RenderError> {
        let event = match self {
            Self::PreflightRecord {
                event,
                json_pointer,
                working_bytes,
                work_units,
            } => NormalizationEvent::PreflightRecord {
                event,
                json_pointer,
                working_bytes: *working_bytes,
                work_units: *work_units,
            },
            Self::CommitRecord {
                event,
                element_id,
                local_owner,
                json_pointer,
                working_bytes,
                work_units,
            } => NormalizationEvent::CommitRecord {
                event,
                element_id: element_id.as_deref(),
                local_owner: local_owner.as_deref(),
                json_pointer,
                working_bytes: *working_bytes,
                work_units: *work_units,
            },
            Self::Diagnostic {
                diagnostic,
                local_owner,
                source_order,
                source_position,
            } => NormalizationEvent::Diagnostic {
                diagnostic: diagnostic.clone(),
                local_owner: local_owner.as_deref(),
                source_order: *source_order,
                source_position: *source_position,
            },
        };
        session.observe_normalization_event(request, event)
    }
}

impl PreparedRelationshipEvent {
    fn record(
        events: &mut Vec<Self>,
        event: RelationshipEvent<'_>,
    ) -> Result<(), FramePreparationError> {
        let event = match event {
            RelationshipEvent::SearchStep {
                element_id,
                json_pointer,
                attempted_depth,
            } => Self::SearchStep {
                element_id: try_owned(element_id)
                    .map_err(|_| FramePreparationError::failed("relationship_journal"))?,
                json_pointer: try_owned(json_pointer)
                    .map_err(|_| FramePreparationError::failed("relationship_journal"))?,
                attempted_depth,
            },
            RelationshipEvent::Traversal(traversal) => Self::Traversal {
                element_id: try_owned(traversal.element_id)
                    .map_err(|_| FramePreparationError::failed("relationship_journal"))?,
                local_owner: try_owned(traversal.local_owner)
                    .map_err(|_| FramePreparationError::failed("relationship_journal"))?,
                json_pointer: try_owned(traversal.json_pointer)
                    .map_err(|_| FramePreparationError::failed("relationship_journal"))?,
                attempted_depth: traversal.attempted_depth,
                retained: traversal.retained,
            },
        };
        events
            .try_reserve(1)
            .map_err(|_| FramePreparationError::failed("relationship_journal"))?;
        events.push(event);
        Ok(())
    }

    pub(super) fn replay(
        &self,
        session: &mut RenderSession,
        request: &RenderRequest,
    ) -> Result<(), RenderError> {
        match self {
            Self::SearchStep {
                element_id,
                json_pointer,
                attempted_depth,
            } => session.observe_relationship_event(
                request,
                RelationshipEvent::SearchStep {
                    element_id,
                    json_pointer,
                    attempted_depth: *attempted_depth,
                },
            ),
            Self::Traversal {
                element_id,
                local_owner,
                json_pointer,
                attempted_depth,
                retained,
            } => session.observe_relationship_event(
                request,
                RelationshipEvent::Traversal(RelationshipTraversal {
                    element_id,
                    local_owner,
                    json_pointer,
                    attempted_depth: *attempted_depth,
                    retained: *retained,
                }),
            ),
        }
    }
}

impl RenderSession {
    pub(super) fn observe_original_json_event(
        &mut self,
        request: &RenderRequest,
        kind: OriginalJsonEntryKind,
        json_pointer: &str,
        is_element_occurrence: bool,
    ) -> Result<(), RenderError> {
        let event = match kind {
            OriginalJsonEntryKind::Node => "json_node",
            OriginalJsonEntryKind::Member => "json_member",
        };
        self.transact(
            request,
            ResourceContext::new(
                ResourcePhase::JsonTraversalAndNormalization,
                event,
                json_pointer,
            ),
            ResourceDelta {
                elements: u64::from(is_element_occurrence),
                total_work_units: 8,
                ..ResourceDelta::default()
            },
        )
    }

    pub(super) fn observe_normalization_event(
        &mut self,
        request: &RenderRequest,
        event: NormalizationEvent<'_>,
    ) -> Result<(), RenderError> {
        match event {
            NormalizationEvent::PreflightRecord {
                event,
                json_pointer,
                working_bytes,
                work_units,
            } => {
                let delta = ResourceDelta {
                    working_bytes,
                    total_work_units: work_units,
                    ..ResourceDelta::default()
                };
                if self.meter.would_cross_fatal(delta) {
                    let context = ResourceContext::new(
                        ResourcePhase::JsonTraversalAndNormalization,
                        event,
                        json_pointer,
                    );
                    return match self.transact(request, context, delta) {
                        Err(error) => Err(error),
                        Ok(()) => Err(self.internal_failure(
                            request.format(),
                            "normalization preflight committed unexpectedly",
                            "/elements",
                        )),
                    };
                }
                Ok(())
            }
            NormalizationEvent::CommitRecord {
                event,
                element_id,
                local_owner,
                json_pointer,
                working_bytes,
                work_units,
            } => self.transact(
                request,
                ResourceContext::new(
                    ResourcePhase::JsonTraversalAndNormalization,
                    event,
                    json_pointer,
                )
                .with_element_id(element_id)
                .with_local_owner(local_owner),
                ResourceDelta {
                    working_bytes,
                    total_work_units: work_units,
                    ..ResourceDelta::default()
                },
            ),
            NormalizationEvent::Diagnostic {
                diagnostic,
                local_owner,
                source_order,
                source_position,
            } => {
                if matches!(self.target, RenderTarget::Frame(_)) {
                    self.pending_frame_normalization_diagnostics
                        .try_reserve(1)
                        .map_err(|_| {
                            self.memory_failure(
                                request,
                                "frame_preparation",
                                "normalization_diagnostic",
                            )
                        })?;
                    let local_owner = local_owner.map(try_owned).transpose().map_err(|_| {
                        self.memory_failure(request, "frame_preparation", "diagnostic_owner")
                    })?;
                    self.pending_frame_normalization_diagnostics.push(
                        PendingNormalizationDiagnostic {
                            diagnostic,
                            local_owner,
                            source_order,
                            source_position,
                        },
                    );
                    Ok(())
                } else {
                    self.retain_diagnostic_with_owner(
                        request,
                        ResourcePhase::JsonTraversalAndNormalization,
                        "normalization_repair",
                        diagnostic,
                        DiagnosticOwnership {
                            local_owner,
                            source_order: Some(source_order),
                            source_position: Some(source_position),
                        },
                    )
                }
            }
        }
    }

    pub(super) fn observe_relationship_event(
        &mut self,
        request: &RenderRequest,
        event: RelationshipEvent<'_>,
    ) -> Result<(), RenderError> {
        match event {
            RelationshipEvent::SearchStep {
                element_id,
                json_pointer,
                attempted_depth,
            } => {
                let context = ResourceContext::new(
                    ResourcePhase::AuthorityReferenceCensus,
                    "relationship_search_step",
                    json_pointer,
                )
                .with_element_id(Some(element_id))
                .with_local_owner(Some(element_id));
                let delta = ResourceDelta {
                    observed_relationship_depth: u64::from(attempted_depth),
                    total_work_units: relationships::traversal_delta(false).0,
                    ..ResourceDelta::default()
                };
                self.transact(request, context, delta)
            }
            RelationshipEvent::Traversal(traversal) => {
                let (total_work_units, working_bytes) =
                    relationships::traversal_delta(traversal.retained);
                self.transact(
                    request,
                    ResourceContext::new(
                        ResourcePhase::AuthorityReferenceCensus,
                        "relationship_traversal",
                        traversal.json_pointer,
                    )
                    .with_element_id(Some(traversal.element_id))
                    .with_local_owner(Some(traversal.local_owner)),
                    ResourceDelta {
                        observed_relationship_depth: u64::from(traversal.attempted_depth),
                        working_bytes,
                        total_work_units,
                        ..ResourceDelta::default()
                    },
                )
            }
        }
    }
}
