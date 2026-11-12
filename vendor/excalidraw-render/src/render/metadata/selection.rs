//! Metadata selection: the session decides, component by component, which
//! optional artifact metadata the artifact retains under the request's resource
//! limits. Each candidate is estimated, generated and measured against the
//! meter's optional-metadata crossing; a component that does not fit is
//! diagnosed as omitted rather than failing the render, while the mandatory
//! font notice must fit or the render fails. Charge order is canonical.

use crate::diagnostic_registry::MetadataFailure;
use crate::outcome::PendingDiagnostic;
use crate::resource_accounting::{
    OptionalMetadataCrossing, ResourceContext, ResourceDelta, ResourcePhase,
};
use crate::scene::SourceField;
use crate::{ErrorCode, MetadataComponent, RenderError, RenderRequest, SceneDocument};

use super::super::diagnostics::{
    metadata_failure_diagnostic, optional_metadata_omitted_diagnostic,
};
use super::super::resolve::artifact_retention_delta;
use super::super::{notices, DiagnosticOwnership, FormatEncodeError, RenderSession, RenderTarget};
use super::{ArtifactMetadata, ManifestTarget, MetadataEstimate, MetadataSelection};

fn manifest_target<'a>(target: RenderTarget, scene: &'a SceneDocument) -> ManifestTarget<'a> {
    match target {
        RenderTarget::Scene => ManifestTarget::Scene,
        RenderTarget::Frame(target) => {
            let name = scene
                .element_at(target.source_index)
                .and_then(|source| match source.name {
                    SourceField::Value(name) => name,
                    SourceField::Missing | SourceField::Invalid => None,
                });
            ManifestTarget::Frame {
                ordinal: target.ordinal,
                kind: target.kind,
                name,
            }
        }
    }
}

#[derive(Clone, Copy)]
enum OptionalMetadataComponent {
    Provenance,
    DiagnosticSummary,
    EmbeddedScene,
}

pub(crate) trait ArtifactMeasurement {
    const DEFERS_MANDATORY_OUTPUT_ADMISSION: bool;

    fn output_len(&self) -> usize;
}

impl ArtifactMeasurement for usize {
    const DEFERS_MANDATORY_OUTPUT_ADMISSION: bool = true;

    fn output_len(&self) -> usize {
        *self
    }
}

impl ArtifactMeasurement for Vec<u8> {
    const DEFERS_MANDATORY_OUTPUT_ADMISSION: bool = false;

    fn output_len(&self) -> usize {
        self.len()
    }
}

impl OptionalMetadataComponent {
    fn requested(self, selection: MetadataSelection) -> bool {
        match self {
            Self::Provenance => selection.provenance,
            Self::DiagnosticSummary => selection.diagnostic_summary,
            Self::EmbeddedScene => selection.embedded_scene,
        }
    }

    fn enable(self, selection: &mut MetadataSelection) {
        match self {
            Self::Provenance => selection.provenance = true,
            Self::DiagnosticSummary => selection.diagnostic_summary = true,
            Self::EmbeddedScene => selection.embedded_scene = true,
        }
    }

    const fn event(self) -> &'static str {
        match self {
            Self::Provenance => "provenance_metadata_omitted",
            Self::DiagnosticSummary => "diagnostic_summary_metadata_omitted",
            Self::EmbeddedScene => "embedded_scene_metadata_omitted",
        }
    }

    const fn measurement_event(self) -> &'static str {
        match self {
            Self::Provenance => "provenance_metadata_measurement",
            Self::DiagnosticSummary => "diagnostic_summary_metadata_measurement",
            Self::EmbeddedScene => "embedded_scene_metadata_measurement",
        }
    }

    const fn retention_event(self) -> &'static str {
        match self {
            Self::Provenance => "provenance_metadata_retention",
            Self::DiagnosticSummary => "diagnostic_summary_metadata_retention",
            Self::EmbeddedScene => "embedded_scene_metadata_retention",
        }
    }
}

impl RenderSession {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn select_artifact_metadata<M: ArtifactMeasurement>(
        &mut self,
        scene: &SceneDocument,
        request: &RenderRequest,
        mandatory_notices: Option<&notices::MandatoryNotices>,
        error_code: ErrorCode,
        codec_emission_work: u64,
        mut measure: impl FnMut(&ArtifactMetadata) -> Result<M, FormatEncodeError>,
    ) -> Result<(ArtifactMetadata, M), RenderError> {
        let nested_notices = if self.report.diagnostics().iter().any(|d| {
            matches!(
                d.details(),
                Some(crate::DiagnosticDetails::NestedResourceLimit {
                    recovery: crate::Recovery::Placeholder,
                    ..
                })
            ) || (d.code() == crate::DiagnosticCode::IMAGE_FORBIDDEN_RESOURCE
                && matches!(
                    d.details(),
                    Some(
                        crate::DiagnosticDetails::Asset {
                            recovery: crate::Recovery::Placeholder,
                            ..
                        } | crate::DiagnosticDetails::NestedSource {
                            recovery: crate::Recovery::Placeholder,
                            ..
                        }
                    )
                ))
        }) {
            let existing = mandatory_notices
                .map(super::super::notices::MandatoryNotices::copy_from)
                .transpose()
                .map_err(|_| {
                    self.memory_failure(request, "artifact_manifest", "nested_resource_notice")
                })?;
            Some(
                super::super::notices::MandatoryNotices::with_error_image_recipe(
                    existing,
                    request.format(),
                )
                .map_err(|_| {
                    self.memory_failure(request, "artifact_manifest", "nested_resource_notice")
                })?,
            )
        } else {
            None
        };
        let mandatory_notices = nested_notices.as_ref().or(mandatory_notices);
        let snapshot = self.report.clone();
        let manifest_target = manifest_target(self.target, scene);
        let requested = MetadataSelection::requested(request);
        let mut accepted = MetadataSelection::default();
        let baseline_estimate = ArtifactMetadata::estimate(
            scene,
            self.profile,
            &snapshot,
            mandatory_notices,
            accepted,
            manifest_target,
        );
        let baseline_context = ResourceContext::new(
            ResourcePhase::Metadata,
            "mandatory_notice_metadata_generation",
            "",
        );
        if mandatory_notices.is_some() {
            let baseline_delta = ResourceDelta {
                working_bytes: baseline_estimate.scratch_bytes,
                total_work_units: baseline_estimate.generation_work,
                ..ResourceDelta::default()
            };
            if self
                .meter
                .optional_metadata_crossing(baseline_context, baseline_delta)
                .is_some()
            {
                return Err(self.format_failure(
                    request,
                    error_code,
                    FormatEncodeError::Encoding(
                        "mandatory notice metadata cannot be generated".to_owned(),
                    ),
                ));
            }
            self.transact(request, baseline_context, baseline_delta)?;
        }
        #[cfg(test)]
        let _baseline_fault_context = super::test_faults::context(manifest_target, accepted);
        let baseline = ArtifactMetadata::build(
            scene,
            self.profile,
            &snapshot,
            mandatory_notices,
            accepted,
            manifest_target,
        )
        .map_err(|error| self.format_failure(request, error_code, error))?;
        let baseline_artifact =
            measure(&baseline).map_err(|error| self.format_failure(request, error_code, error))?;
        let baseline_bytes = u64::try_from(baseline_artifact.output_len()).unwrap_or(u64::MAX);
        if self
            .meter
            .optional_metadata_crossing(
                ResourceContext::new(
                    ResourcePhase::Metadata,
                    "mandatory_notice_metadata_retention",
                    "",
                ),
                artifact_retention_delta(baseline_bytes, codec_emission_work),
            )
            .is_some()
        {
            if mandatory_notices.is_some() && !M::DEFERS_MANDATORY_OUTPUT_ADMISSION {
                return Err(self.format_failure(
                    request,
                    error_code,
                    FormatEncodeError::Encoding(
                        "mandatory notice cannot be retained in the artifact".to_owned(),
                    ),
                ));
            }
            if mandatory_notices.is_none() {
                return Ok((baseline, baseline_artifact));
            }
        }
        if mandatory_notices.is_some() {
            self.release_working(request, baseline_context, baseline_estimate.scratch_bytes)?;
        }
        let mut accepted_metadata = baseline;
        let mut accepted_artifact = baseline_artifact;
        let mut accepted_estimate = if mandatory_notices.is_some() {
            baseline_estimate
        } else {
            MetadataEstimate::default()
        };

        for component in [
            OptionalMetadataComponent::Provenance,
            OptionalMetadataComponent::DiagnosticSummary,
            OptionalMetadataComponent::EmbeddedScene,
        ] {
            if !component.requested(requested) {
                continue;
            }
            let mut candidate_selection = accepted;
            component.enable(&mut candidate_selection);
            let candidate_estimate = ArtifactMetadata::estimate(
                scene,
                self.profile,
                &snapshot,
                mandatory_notices,
                candidate_selection,
                manifest_target,
            );
            let compression_workspace = u64::from(matches!(
                component,
                OptionalMetadataComponent::EmbeddedScene
            )) * 1_048_576;
            let scratch_bytes = candidate_estimate
                .scratch_bytes
                .saturating_sub(accepted_estimate.scratch_bytes)
                .saturating_add(compression_workspace);
            let generation_work = candidate_estimate
                .generation_work
                .saturating_sub(accepted_estimate.generation_work);
            let generation_delta = ResourceDelta {
                working_bytes: scratch_bytes,
                total_work_units: generation_work,
                ..ResourceDelta::default()
            };
            if let Some(crossing) = self.meter.optional_metadata_crossing(
                ResourceContext::new(ResourcePhase::Metadata, component.event(), ""),
                generation_delta,
            ) {
                let diagnostic = self.optional_metadata_omitted(request, crossing)?;
                self.retain_metadata_diagnostic(request, component, component.event(), diagnostic)?;
                continue;
            }
            let generation_context =
                ResourceContext::new(ResourcePhase::Metadata, component.event(), "");
            self.transact(request, generation_context, generation_delta)?;
            #[cfg(test)]
            let _candidate_fault_context =
                super::test_faults::context(manifest_target, candidate_selection);
            let candidate = match ArtifactMetadata::build(
                scene,
                self.profile,
                &snapshot,
                mandatory_notices,
                candidate_selection,
                manifest_target,
            ) {
                Ok(candidate) => candidate,
                Err(error @ FormatEncodeError::MemoryUnavailable { .. }) => {
                    return Err(self.format_failure(request, error_code, error));
                }
                Err(error @ FormatEncodeError::SvgFontSubset { .. })
                | Err(error @ FormatEncodeError::FontUnavailable(_)) => {
                    return Err(self.format_failure(request, error_code, error));
                }
                Err(FormatEncodeError::Metadata(failure)) => {
                    self.release_working(request, generation_context, scratch_bytes)?;
                    if failure.component() == MetadataComponent::Manifest {
                        if mandatory_notices.is_some() {
                            return Err(self.format_failure(
                                request,
                                error_code,
                                FormatEncodeError::Encoding(
                                    "mandatory notice manifest could not be serialized".to_owned(),
                                ),
                            ));
                        }
                        let diagnostic = self.metadata_omitted(request, failure)?;
                        self.retain_metadata_diagnostic(
                            request,
                            component,
                            "artifact_manifest_metadata_omitted",
                            diagnostic,
                        )?;
                        let empty = ArtifactMetadata::empty();
                        let artifact = measure(&empty)
                            .map_err(|error| self.format_failure(request, error_code, error))?;
                        return Ok((empty, artifact));
                    }
                    let diagnostic = self.metadata_omitted(request, failure)?;
                    self.retain_metadata_diagnostic(
                        request,
                        component,
                        component.event(),
                        diagnostic,
                    )?;
                    continue;
                }
                Err(error @ FormatEncodeError::Encoding(_)) => {
                    self.release_working(request, generation_context, scratch_bytes)?;
                    return Err(self.format_failure(request, error_code, error));
                }
            };
            let measurement_delta = ResourceDelta {
                total_work_units: generation_work,
                ..ResourceDelta::default()
            };
            if let Some(crossing) = self.meter.optional_metadata_crossing(
                ResourceContext::new(ResourcePhase::Metadata, component.measurement_event(), ""),
                measurement_delta,
            ) {
                self.release_working(request, generation_context, scratch_bytes)?;
                let diagnostic = self.optional_metadata_omitted(request, crossing)?;
                self.retain_metadata_diagnostic(request, component, component.event(), diagnostic)?;
                continue;
            }
            self.transact(
                request,
                ResourceContext::new(ResourcePhase::Metadata, component.measurement_event(), ""),
                measurement_delta,
            )?;
            let measured = measure(&candidate);
            let candidate_artifact = match measured {
                Ok(candidate_artifact) => candidate_artifact,
                Err(error @ FormatEncodeError::MemoryUnavailable { .. }) => {
                    return Err(self.format_failure(request, error_code, error));
                }
                Err(error @ FormatEncodeError::SvgFontSubset { .. })
                | Err(error @ FormatEncodeError::FontUnavailable(_)) => {
                    return Err(self.format_failure(request, error_code, error));
                }
                Err(FormatEncodeError::Metadata(failure)) => {
                    self.release_working(request, generation_context, scratch_bytes)?;
                    if failure.component() == MetadataComponent::Manifest {
                        if mandatory_notices.is_some() {
                            return Err(self.format_failure(
                                request,
                                error_code,
                                FormatEncodeError::Encoding(
                                    "mandatory notice manifest could not be embedded".to_owned(),
                                ),
                            ));
                        }
                        let diagnostic = self.metadata_omitted(request, failure)?;
                        self.retain_metadata_diagnostic(
                            request,
                            component,
                            "artifact_manifest_metadata_omitted",
                            diagnostic,
                        )?;
                        let empty = ArtifactMetadata::empty();
                        let artifact = measure(&empty)
                            .map_err(|error| self.format_failure(request, error_code, error))?;
                        return Ok((empty, artifact));
                    }
                    let diagnostic = self.metadata_omitted(request, failure)?;
                    self.retain_metadata_diagnostic(
                        request,
                        component,
                        component.event(),
                        diagnostic,
                    )?;
                    continue;
                }
                Err(error @ FormatEncodeError::Encoding(_)) => {
                    self.release_working(request, generation_context, scratch_bytes)?;
                    return Err(self.format_failure(request, error_code, error));
                }
            };
            let marginal_bytes = candidate_artifact
                .output_len()
                .checked_sub(accepted_artifact.output_len())
                .and_then(|bytes| u64::try_from(bytes).ok())
                .ok_or_else(|| {
                    self.internal_failure(
                        request.format(),
                        "optional metadata reduced the encoded artifact length",
                        "",
                    )
                })?;
            let output_bytes = u64::try_from(accepted_artifact.output_len())
                .unwrap_or(u64::MAX)
                .saturating_add(marginal_bytes);
            if let Some(crossing) = self.meter.optional_metadata_crossing(
                ResourceContext::new(ResourcePhase::Metadata, component.retention_event(), ""),
                artifact_retention_delta(output_bytes, codec_emission_work),
            ) {
                self.release_working(request, generation_context, scratch_bytes)?;
                let diagnostic = self.optional_metadata_omitted(request, crossing)?;
                self.retain_metadata_diagnostic(request, component, component.event(), diagnostic)?;
            } else {
                self.release_working(request, generation_context, scratch_bytes)?;
                accepted = candidate_selection;
                accepted_metadata = candidate;
                accepted_artifact = candidate_artifact;
                accepted_estimate = candidate_estimate;
            }
        }

        Ok((accepted_metadata, accepted_artifact))
    }

    fn retain_metadata_diagnostic(
        &mut self,
        request: &RenderRequest,
        component: OptionalMetadataComponent,
        event: &'static str,
        diagnostic: PendingDiagnostic,
    ) -> Result<(), RenderError> {
        // Root metadata events have semantic component order, not lexical code
        // order (which would put diagnostic-summary before provenance).
        let position = match component {
            OptionalMetadataComponent::Provenance => 0,
            OptionalMetadataComponent::DiagnosticSummary => 1,
            OptionalMetadataComponent::EmbeddedScene => 2,
        };
        self.retain_diagnostic_with_owner(
            request,
            ResourcePhase::Metadata,
            event,
            diagnostic,
            DiagnosticOwnership {
                local_owner: None,
                source_order: None,
                source_position: Some(position),
            },
        )
    }

    /// The `resource.optional-metadata-omitted` diagnostic for `crossing`,
    /// or the memory failure when it cannot be built.
    fn optional_metadata_omitted(
        &mut self,
        request: &RenderRequest,
        crossing: OptionalMetadataCrossing,
    ) -> Result<PendingDiagnostic, RenderError> {
        optional_metadata_omitted_diagnostic(request.format(), crossing).map_err(|_| {
            self.memory_failure(request, "compatibility_report", "optional_metadata_omitted")
        })
    }

    /// The `metadata.*` diagnostic for the omitted component `failure`, or
    /// the memory failure when it cannot be built.
    fn metadata_omitted(
        &mut self,
        request: &RenderRequest,
        failure: MetadataFailure,
    ) -> Result<PendingDiagnostic, RenderError> {
        metadata_failure_diagnostic(request.format(), failure)
            .map_err(|_| self.memory_failure(request, "compatibility_report", "metadata_omitted"))
    }
}
