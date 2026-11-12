//! Fallback accounting over the resolved graph: the post-assembly pass that
//! charges frame clips, image corner clips, text, frames and compatibility
//! fallbacks in paint order, prunes what a local crossing omitted, and settles
//! the fonts and mandatory notices the surviving elements need; plus the
//! compatibility-omission charges and the fallback-origin queries it relies on.
//! Charge order is the canonical trace order (ADR-0002).

use crate::diagnostic_registry::LocalLimit;
use crate::resource_accounting::{ResourceContext, ResourceDelta, ResourcePhase};
use crate::{
    DiagnosticCode, DiagnosticDetails, Recovery, RenderError, RenderRequest, ResourceValue,
};

use super::super::diagnostics::{
    fallback_font_diagnostic, official_missing_glyph_diagnostic, FontFallback,
};
use super::super::resolve::{
    format_error_code, ElementLocalCrossing, ResolvedElement, ResolvedFallback, ResolvedScene,
    SemanticWorkingRecord,
};
use super::super::{text, try_owned, DiagnosticOwnership, FormatEncodeError, RenderSession};

impl RenderSession {
    pub(crate) fn account_fallback_semantics(
        &mut self,
        resolved: &mut ResolvedScene,
        request: &RenderRequest,
        semantic_working: &mut Vec<SemanticWorkingRecord>,
    ) -> Result<(), RenderError> {
        resolved
            .coordinate_omissions
            .sort_unstable_by_key(|recovery| recovery.paint_order);
        let mut recoveries = std::mem::take(&mut resolved.coordinate_omissions)
            .into_iter()
            .peekable();
        for element in &mut resolved.elements {
            while recoveries
                .peek()
                .is_some_and(|recovery| recovery.paint_order <= element.paint_order())
            {
                self.account_coordinate_omission(
                    recoveries.next().expect("peeked recovery"),
                    request,
                )?;
            }
            if let ResolvedElement::Outline(outline) = element {
                self.account_embeddable_label(outline, request)?;
            }
            let direct_clip_source = match element {
                ResolvedElement::Outline(outline) if !outline.frame_clips.is_empty() => Some((
                    outline.source.id(),
                    outline.source.pointer(),
                    &outline.frame_clips,
                )),
                ResolvedElement::Freedraw(freedraw) if !freedraw.frame_clips.is_empty() => Some((
                    freedraw.source.id(),
                    freedraw.source.pointer(),
                    &freedraw.frame_clips,
                )),
                _ => None,
            };
            if let Some((element_id, json_pointer, clips)) = direct_clip_source {
                self.transact(
                    request,
                    ResourceContext::new(
                        ResourcePhase::SharedSemanticResolution,
                        "frame_clip_geometry",
                        json_pointer,
                    )
                    .with_element_id(Some(element_id))
                    .with_local_owner(Some(element_id)),
                    super::super::resolve::frame_clip_delta(clips),
                )?;
                continue;
            }
            if let ResolvedElement::Image(image) = element {
                // A rounded image's corner clip is geometry the reference
                // builds for every format; it is charged like a frame clip's,
                // once per target, so the ledger agrees across formats and
                // replays.
                if let Some(clip) = &image.corner_clip {
                    let geometry_segments =
                        u64::try_from(clip.path.operations.len()).unwrap_or(u64::MAX);
                    self.transact(
                        request,
                        ResourceContext::new(
                            ResourcePhase::SharedSemanticResolution,
                            "image_corner_clip_geometry",
                            image.source.pointer(),
                        )
                        .with_element_id(Some(image.source.id()))
                        .with_local_owner(Some(image.source.id())),
                        ResourceDelta {
                            geometry_segments,
                            total_work_units: geometry_segments.saturating_mul(32),
                            ..ResourceDelta::default()
                        },
                    )?;
                }
                continue;
            }
            if let ResolvedElement::Text(text) = element {
                self.account_resolved_text(text, request, semantic_working)?;
                continue;
            }
            if let ResolvedElement::Frame(frame) = element {
                let outline_segments = frame.outline.as_ref().map_or(0, |outline| {
                    u64::try_from(outline.path.operations.len()).unwrap_or(u64::MAX)
                });
                let clip_segments = frame.frame_clips.iter().fold(0_u64, |total, clip| {
                    total.saturating_add(
                        u64::try_from(clip.path.operations.len()).unwrap_or(u64::MAX),
                    )
                });
                let label_segments = frame.name.as_ref().map_or(0, |name| {
                    u64::try_from(name.label.path.operations.len()).unwrap_or(u64::MAX)
                });
                let text_scalars = frame.name.as_ref().map_or(0, |name| {
                    u64::try_from(name.label.text.chars().count()).unwrap_or(u64::MAX)
                });
                let geometry_segments = outline_segments
                    .saturating_add(clip_segments)
                    .saturating_add(label_segments);
                let total_work_units = geometry_segments
                    .saturating_mul(32)
                    .saturating_add(text_scalars.saturating_mul(32));
                let working_bytes = 128_u64
                    .saturating_add(geometry_segments.saturating_mul(64))
                    .saturating_add(text_scalars.saturating_mul(4));
                let delta = ResourceDelta {
                    geometry_segments,
                    text_scalars,
                    working_bytes,
                    total_work_units,
                    ..ResourceDelta::default()
                };
                let context = ResourceContext::new(
                    ResourcePhase::SharedSemanticResolution,
                    "frame_geometry_and_name",
                    frame.source.pointer(),
                )
                .with_element_id(Some(frame.source.id()))
                .with_local_owner(Some(frame.source.id()));
                if let Some(name) = &frame.name {
                    for codepoint in &name.label.missing_codepoints {
                        let diagnostic = official_missing_glyph_diagnostic(
                            request.format(),
                            frame.source.id(),
                            frame.source.pointer(),
                            *codepoint,
                        )
                        .map_err(|_| {
                            self.memory_failure(
                                request,
                                "compatibility_report",
                                "frame_name_missing_glyph",
                            )
                        })?;
                        self.retain_diagnostic_with_owner(
                            request,
                            ResourcePhase::SharedSemanticResolution,
                            "frame_name_missing_glyph",
                            diagnostic,
                            DiagnosticOwnership {
                                local_owner: Some(frame.source.id()),
                                source_order: Some(frame.source_order),
                                source_position: Some(u64::MAX),
                            },
                        )?;
                    }
                }
                self.transact(request, context, delta)?;
                semantic_working.try_reserve(1).map_err(|_| {
                    self.memory_failure(request, "semantic_geometry", "frame_working_record")
                })?;
                semantic_working.push(SemanticWorkingRecord {
                    source_order: frame.source_order,
                    event: "resolved_frame_geometry",
                    element_id: Some(try_owned(frame.source.id()).map_err(|_| {
                        self.memory_failure(request, "semantic_geometry", "frame_identity")
                    })?),
                    local_owner: Some(try_owned(frame.source.id()).map_err(|_| {
                        self.memory_failure(request, "semantic_geometry", "frame_owner")
                    })?),
                    json_pointer: try_owned(frame.source.pointer()).map_err(|_| {
                        self.memory_failure(request, "semantic_geometry", "frame_pointer")
                    })?,
                    bytes: working_bytes,
                });
                continue;
            }
            let ResolvedElement::Fallback(fallback) = element else {
                continue;
            };
            let clip_segments = fallback.frame_clips.iter().fold(0_u64, |total, clip| {
                total.saturating_add(u64::try_from(clip.path.operations.len()).unwrap_or(u64::MAX))
            });
            let mut geometry_segments = fallback
                .geometry_command_count()
                .saturating_add(clip_segments);
            let mut glyphs = fallback.label.as_ref().map_or(0, |label| label.glyph_count);
            let mut working_bytes = 128_u64.saturating_add(geometry_segments.saturating_mul(64));
            let mut total_work_units = geometry_segments
                .saturating_mul(32)
                .saturating_add(glyphs.saturating_mul(32));
            let mut delta = ResourceDelta {
                geometry_segments,
                working_bytes,
                total_work_units,
                ..ResourceDelta::default()
            };
            let context = ResourceContext::new(
                ResourcePhase::SharedSemanticResolution,
                "compatibility_fallback_geometry",
                &fallback.source.pointer,
            )
            .with_element_id(fallback.source.id.as_deref())
            .with_local_owner(fallback.source.id.as_deref());
            if self.meter.would_cross_fatal(delta) {
                self.transact(request, context, delta)?;
                return Err(self.internal_failure(
                    request.format(),
                    "fatal fallback resource preflight committed unexpectedly",
                    "/elements",
                ));
            }
            let local_crossing = if geometry_segments
                > u64::from(
                    request
                        .resource_limits()
                        .max_geometry_segments_per_element(),
                ) {
                Some(ElementLocalCrossing {
                    limit: LocalLimit::GeometrySegmentsPerElement,
                    configured: ResourceValue::Integer(u64::from(
                        request
                            .resource_limits()
                            .max_geometry_segments_per_element(),
                    )),
                    attempted: crate::Measurement::Attempted {
                        value: ResourceValue::Integer(geometry_segments),
                    },
                    pointer_field: None,
                    message:
                        "compatibility fallback omitted because its geometry exceeds its limit",
                })
            } else if total_work_units > request.resource_limits().max_element_work_units() {
                Some(ElementLocalCrossing {
                    limit: LocalLimit::ElementWorkUnits,
                    configured: ResourceValue::Integer(
                        request.resource_limits().max_element_work_units(),
                    ),
                    attempted: crate::Measurement::Attempted {
                        value: ResourceValue::Integer(total_work_units),
                    },
                    pointer_field: None,
                    message:
                        "compatibility fallback omitted because its semantic work exceeds its limit",
                })
            } else {
                None
            };
            if let Some(crossing) = local_crossing {
                self.commit_local_rejection_observation(
                    request,
                    context,
                    ResourceDelta {
                        geometry_segments,
                        ..ResourceDelta::default()
                    },
                )?;
                if self.has_resource_fallback_origin(fallback) && fallback.label.is_some() {
                    fallback.label = None;
                    geometry_segments = fallback
                        .geometry_command_count()
                        .saturating_add(clip_segments);
                    glyphs = 0;
                    working_bytes = 128_u64.saturating_add(geometry_segments.saturating_mul(64));
                    total_work_units = geometry_segments.saturating_mul(32);
                    if geometry_segments
                        <= u64::from(
                            request
                                .resource_limits()
                                .max_geometry_segments_per_element(),
                        )
                        && total_work_units <= request.resource_limits().max_element_work_units()
                    {
                        delta = ResourceDelta {
                            geometry_segments,
                            working_bytes,
                            total_work_units,
                            ..ResourceDelta::default()
                        };
                    } else if self.omit_failed_resource_fallback(fallback) {
                        self.charge_local_omission(
                            request,
                            fallback.source.id.as_deref(),
                            &fallback.source.pointer,
                            "fallback_local_omission",
                        )?;
                        fallback.render = false;
                        continue;
                    }
                } else if self.omit_failed_resource_fallback(fallback) {
                    self.charge_local_omission(
                        request,
                        fallback.source.id.as_deref(),
                        &fallback.source.pointer,
                        "fallback_local_omission",
                    )?;
                    fallback.render = false;
                    continue;
                } else {
                    let diagnostic = crossing
                        .diagnostic(
                            request.format(),
                            fallback.source.id.as_deref(),
                            &fallback.source.pointer,
                        )
                        .map_err(|_| {
                            self.memory_failure(
                                request,
                                "compatibility_report",
                                "fallback_local_recovery",
                            )
                        })?;
                    self.retain_diagnostic_with_owner(
                        request,
                        ResourcePhase::SharedSemanticResolution,
                        "fallback_local_resource_recovery",
                        diagnostic,
                        DiagnosticOwnership {
                            local_owner: fallback.source.id.as_deref(),
                            source_order: Some(fallback.source_order),
                            source_position: Some(0),
                        },
                    )?;
                    self.charge_local_omission(
                        request,
                        fallback.source.id.as_deref(),
                        &fallback.source.pointer,
                        "fallback_local_omission",
                    )?;
                    fallback.render = false;
                    continue;
                }
            }
            self.retain_fallback_font_diagnostics(request, fallback)?;
            self.transact(request, context, delta)?;
            semantic_working.try_reserve(1).map_err(|_| {
                self.memory_failure(request, "semantic_geometry", "fallback_working_record")
            })?;
            semantic_working.push(SemanticWorkingRecord {
                source_order: fallback.source_order,
                event: "resolved_compatibility_fallback",
                element_id: fallback
                    .source
                    .id
                    .as_deref()
                    .map(try_owned)
                    .transpose()
                    .map_err(|_| {
                        self.memory_failure(request, "semantic_geometry", "fallback_identity")
                    })?,
                local_owner: fallback
                    .source
                    .id
                    .as_deref()
                    .map(try_owned)
                    .transpose()
                    .map_err(|_| {
                        self.memory_failure(request, "semantic_geometry", "fallback_owner")
                    })?,
                json_pointer: try_owned(&fallback.source.pointer).map_err(|_| {
                    self.memory_failure(request, "semantic_geometry", "fallback_pointer")
                })?,
                bytes: working_bytes,
            });
        }
        for recovery in recoveries {
            self.account_coordinate_omission(recovery, request)?;
        }
        resolved.elements.retain(|element| {
            !matches!(element, ResolvedElement::Fallback(fallback) if !fallback.render)
                && !matches!(element, ResolvedElement::Text(text) if !text.render)
        });
        let has_fallback_label = resolved.elements.iter().any(|element| match element {
            ResolvedElement::Frame(frame) => frame.name.is_some(),
            ResolvedElement::Fallback(fallback) => fallback.label.is_some(),
            _ => false,
        });
        let has_official_text = resolved.elements.iter().any(|element| match element {
            ResolvedElement::Text(_) => true,
            ResolvedElement::Outline(outline) => outline.generated_label.is_some(),
            _ => false,
        });
        if !has_fallback_label {
            resolved.fallback_font_woff2 = None;
            resolved.fallback_font_sfnt = None;
        }
        if !has_official_text {
            resolved.official_fonts.clear();
        }
        resolved.mandatory_notices = if has_fallback_label || has_official_text {
            Some(
                text::mandatory_notices_json(has_fallback_label, &resolved.official_fonts)
                    .map_err(|error| match error {
                        text::LabelError::FontUnavailable(font) => {
                            self.font_unavailable_failure(request.format(), font)
                        }
                        text::LabelError::Memory => self.memory_failure(
                            request,
                            "artifact_manifest",
                            "font_notice_manifest",
                        ),
                        text::LabelError::InvalidFont => self.format_failure(
                            request,
                            format_error_code(request.format()),
                            FormatEncodeError::Encoding(
                                "the mandatory font notice could not be resolved".to_owned(),
                            ),
                        ),
                    })?,
            )
        } else {
            None
        };
        semantic_working.sort_unstable_by(SemanticWorkingRecord::release_order);
        Ok(())
    }

    fn account_coordinate_omission(
        &mut self,
        recovery: super::super::resolve::admission::CoordinateOmission,
        request: &RenderRequest,
    ) -> Result<(), RenderError> {
        let context = ResourceContext::new(
            ResourcePhase::SharedSemanticResolution,
            "derived_coordinate_geometry",
            &recovery.pointer,
        )
        .with_element_id(recovery.id.as_deref())
        .with_local_owner(recovery.id.as_deref());
        if self.meter.would_cross_fatal(recovery.delta) {
            self.transact(request, context, recovery.delta)?;
            return Err(self.internal_failure(
                request.format(),
                "fatal coordinate preflight committed unexpectedly",
                "/elements",
            ));
        }
        if recovery.delta.geometry_segments != 0 || recovery.delta.text_scalars != 0 {
            self.commit_local_rejection_observation(
                request,
                context,
                ResourceDelta {
                    geometry_segments: recovery.delta.geometry_segments,
                    text_scalars: recovery.delta.text_scalars,
                    ..ResourceDelta::default()
                },
            )?;
        }
        for diagnostic in &mut self.report.diagnostics {
            if diagnostic.element_id() == recovery.id.as_deref()
                && diagnostic.json_pointer() == recovery.pointer
            {
                match diagnostic.details_mut() {
                    Some(DiagnosticDetails::Compatibility {
                        recovery,
                        recipe_id,
                    }) if *recovery == Recovery::Placeholder => {
                        *recovery = Recovery::Omitted;
                        *recipe_id = None;
                    }
                    Some(DiagnosticDetails::ResourceLimit {
                        recovery: Some(recovery),
                        ..
                    }) if *recovery == Recovery::Placeholder => {
                        *recovery = Recovery::Omitted;
                    }
                    _ => {}
                }
            }
        }
        if let Some(diagnostic) = recovery.diagnostic {
            self.retain_diagnostic_with_owner(
                request,
                ResourcePhase::SharedSemanticResolution,
                "derived_coordinate_recovery",
                diagnostic,
                DiagnosticOwnership {
                    local_owner: recovery.id.as_deref(),
                    source_order: Some(recovery.source_order),
                    source_position: Some(u64::MAX),
                },
            )?;
        }
        self.charge_local_omission(
            request,
            recovery.id.as_deref(),
            &recovery.pointer,
            "derived_coordinate_omission",
        )
    }

    fn retain_fallback_font_diagnostics(
        &mut self,
        request: &RenderRequest,
        fallback: &ResolvedFallback,
    ) -> Result<(), RenderError> {
        let Some(label) = &fallback.label else {
            return Ok(());
        };
        let substitution = fallback_font_diagnostic(
            FontFallback::GeneratedLabel,
            request.format(),
            fallback.source.id.as_deref(),
            &fallback.source.pointer,
        )
        .map_err(|_| {
            self.memory_failure(
                request,
                "compatibility_report",
                "fallback_font_substitution",
            )
        })?;
        self.retain_diagnostic_with_owner(
            request,
            ResourcePhase::SharedSemanticResolution,
            "fallback_font_substitution",
            substitution,
            DiagnosticOwnership {
                local_owner: fallback.source.id.as_deref(),
                source_order: Some(fallback.source_order),
                source_position: Some(u64::MAX - 1),
            },
        )?;
        for codepoint in &label.missing_codepoints {
            let diagnostic = fallback_font_diagnostic(
                FontFallback::MissingGlyph(*codepoint),
                request.format(),
                fallback.source.id.as_deref(),
                &fallback.source.pointer,
            )
            .map_err(|_| {
                self.memory_failure(request, "compatibility_report", "fallback_missing_glyph")
            })?;
            self.retain_diagnostic_with_owner(
                request,
                ResourcePhase::SharedSemanticResolution,
                "fallback_missing_glyph",
                diagnostic,
                DiagnosticOwnership {
                    local_owner: fallback.source.id.as_deref(),
                    source_order: Some(fallback.source_order),
                    source_position: Some(u64::MAX),
                },
            )?;
        }
        Ok(())
    }

    fn omit_failed_resource_fallback(&mut self, fallback: &ResolvedFallback) -> bool {
        let original = self.report.diagnostics.iter_mut().find(|diagnostic| {
            diagnostic.element_id() == fallback.source.id.as_deref()
                && diagnostic.json_pointer() == fallback.source.pointer
                && matches!(
                    diagnostic.code(),
                    DiagnosticCode::RESOURCE_ELEMENT_COMPLEXITY_REPLACED
                        | DiagnosticCode::RESOURCE_ELEMENT_WORK_EXCEEDED
                        | DiagnosticCode::RESOURCE_COORDINATE_RANGE_EXCEEDED
                )
                && matches!(
                    diagnostic.details(),
                    Some(DiagnosticDetails::ResourceLimit {
                        recovery: Some(Recovery::Placeholder),
                        ..
                    })
                )
        });
        let Some(original) = original else {
            return false;
        };
        let Some(DiagnosticDetails::ResourceLimit { recovery, .. }) = original.details_mut() else {
            return false;
        };
        *recovery = Some(Recovery::Omitted);
        true
    }

    fn has_resource_fallback_origin(&self, fallback: &ResolvedFallback) -> bool {
        self.report.diagnostics.iter().any(|diagnostic| {
            diagnostic.element_id() == fallback.source.id.as_deref()
                && diagnostic.json_pointer() == fallback.source.pointer
                && matches!(
                    diagnostic.code(),
                    DiagnosticCode::RESOURCE_ELEMENT_COMPLEXITY_REPLACED
                        | DiagnosticCode::RESOURCE_ELEMENT_WORK_EXCEEDED
                        | DiagnosticCode::RESOURCE_COORDINATE_RANGE_EXCEEDED
                )
                && matches!(
                    diagnostic.details(),
                    Some(DiagnosticDetails::ResourceLimit {
                        recovery: Some(Recovery::Placeholder),
                        ..
                    })
                )
        })
    }

    pub(crate) fn account_compatibility_omissions(
        &mut self,
        request: &RenderRequest,
    ) -> Result<(), RenderError> {
        let omissions = (|| -> Result<_, std::collections::TryReserveError> {
            let mut omissions = Vec::new();
            for diagnostic in &self.report.diagnostics {
                if !matches!(
                    diagnostic.code(),
                    DiagnosticCode::COMPATIBILITY_UNSUPPORTED_ELEMENT_TYPE
                        | DiagnosticCode::COMPATIBILITY_MALFORMED_ELEMENT
                        | DiagnosticCode::COMPATIBILITY_KNOWN_ELEMENT_REPLACED
                ) || !matches!(
                    diagnostic.details(),
                    Some(DiagnosticDetails::Compatibility {
                        recovery: Recovery::Omitted,
                        ..
                    })
                ) {
                    continue;
                }
                omissions.try_reserve(1)?;
                omissions.push((
                    diagnostic.element_id().map(try_owned).transpose()?,
                    try_owned(diagnostic.json_pointer())?,
                ));
            }
            Ok(omissions)
        })()
        .map_err(|_| self.memory_failure(request, "semantic_omission", "omission_list"))?;
        for (element_id, pointer) in &omissions {
            self.charge_work(
                request,
                ResourceContext::new(
                    ResourcePhase::SharedSemanticResolution,
                    "compatibility_omission",
                    pointer,
                )
                .with_element_id(element_id.as_deref())
                .with_local_owner(element_id.as_deref()),
                8,
            )?;
        }
        Ok(())
    }
}
